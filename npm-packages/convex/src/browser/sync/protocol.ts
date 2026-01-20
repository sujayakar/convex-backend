import type { UserIdentityAttributes } from "../../server/authentication.js";
export type { UserIdentityAttributes } from "../../server/authentication.js";
import { JSONValue, Base64 } from "../../values/index.js";
import { Long } from "../../vendor/long.js";
import * as flexbuffers from "flatbuffers/js/flexbuffers.js";

/**
 * Shared schema
 */

export function u64ToLong(encoded: EncodedU64): U64 {
  const integerBytes = Base64.toByteArray(encoded);
  return Long.fromBytesLE(Array.from(integerBytes));
}

export function longToU64(raw: U64): EncodedU64 {
  const integerBytes = new Uint8Array(raw.toBytesLE());
  return Base64.fromByteArray(integerBytes);
}

export function parseServerMessage(
  encoded: EncodedServerMessage,
): WireServerMessage {
  switch (encoded.type) {
    case "FatalError":
    case "AuthError":
    case "ActionResponse":
    case "TransitionChunk":
    case "Ping": {
      return { ...encoded };
    }
    case "MutationResponse": {
      if (encoded.success) {
        return { ...encoded, ts: u64ToLong(encoded.ts) };
      } else {
        return { ...encoded };
      }
    }
    case "Transition": {
      return {
        ...encoded,
        startVersion: {
          ...encoded.startVersion,
          ts: u64ToLong(encoded.startVersion.ts),
        },
        endVersion: {
          ...encoded.endVersion,
          ts: u64ToLong(encoded.endVersion.ts),
        },
      };
    }
    default: {
      encoded satisfies never;
    }
  }
  return undefined as never;
}

const PACKED_PLACEHOLDER_KEY = "$packed";
const BINARY_FRAME_TYPE_FULL = 0;
const BINARY_FRAME_TYPE_CHUNK = 1;
const binaryTextDecoder = new TextDecoder();

export type BinaryFrame =
  | { type: "full"; payload: Uint8Array }
  | {
      type: "chunk";
      messageId: number;
      partNumber: number;
      totalParts: number;
      payload: Uint8Array;
    };

export function decodeBinaryFrame(data: ArrayBuffer): BinaryFrame {
  const bytes = new Uint8Array(data);
  if (bytes.byteLength === 0) {
    throw new Error("Empty binary frame");
  }
  const frameType = bytes[0];
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (frameType === BINARY_FRAME_TYPE_FULL) {
    return { type: "full", payload: bytes.subarray(1) };
  }
  if (frameType === BINARY_FRAME_TYPE_CHUNK) {
    if (bytes.byteLength < 13) {
      throw new Error("Binary chunk frame too short");
    }
    const messageId = view.getUint32(1, true);
    const partNumber = view.getUint32(5, true);
    const totalParts = view.getUint32(9, true);
    if (totalParts === 0) {
      throw new Error("Binary chunk frame has zero parts");
    }
    if (partNumber >= totalParts) {
      throw new Error("Binary chunk frame part out of range");
    }
    return {
      type: "chunk",
      messageId,
      partNumber,
      totalParts,
      payload: bytes.subarray(13),
    };
  }
  throw new Error(`Unknown binary frame type ${frameType}`);
}

export function parseBinaryServerMessage(payload: Uint8Array): WireServerMessage {
  if (payload.byteLength < 4) {
    throw new Error("Binary server message too short");
  }
  const view = new DataView(
    payload.buffer,
    payload.byteOffset,
    payload.byteLength,
  );
  const headerLength = view.getUint32(0, true);
  const headerStart = 4;
  const headerEnd = headerStart + headerLength;
  if (headerEnd > payload.byteLength) {
    throw new Error("Binary server message header is truncated");
  }
  const headerBytes = payload.subarray(headerStart, headerEnd);
  const headerJson = binaryTextDecoder.decode(headerBytes);
  const header = JSON.parse(headerJson) as EncodedServerMessage;
  const packedValues: unknown[] = [];
  let offset = headerEnd;
  while (offset < payload.byteLength) {
    if (offset + 4 > payload.byteLength) {
      throw new Error("Binary packed value length truncated");
    }
    const length = view.getUint32(offset, true);
    offset += 4;
    const end = offset + length;
    if (end > payload.byteLength) {
      throw new Error("Binary packed value truncated");
    }
    const packedBytes = payload.subarray(offset, end);
    offset = end;
    packedValues.push(decodePackedValue(packedBytes));
  }
  const hydrated = replacePackedValues(header, packedValues) as EncodedServerMessage;
  return parseServerMessage(hydrated);
}

function decodePackedValue(bytes: Uint8Array): unknown {
  const buffer = bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  );
  return normalizeFlexValue(flexbuffers.toObject(buffer));
}

function normalizeFlexValue(value: any): any {
  if (ArrayBuffer.isView(value)) {
    return value.buffer.slice(
      value.byteOffset,
      value.byteOffset + value.byteLength,
    );
  }
  if (value instanceof ArrayBuffer) {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((entry) => normalizeFlexValue(entry));
  }
  if (value && typeof value === "object") {
    const out: Record<string, any> = {};
    for (const [key, entry] of Object.entries(value)) {
      out[key] = normalizeFlexValue(entry);
    }
    return out;
  }
  return value;
}

function replacePackedValues(node: any, packedValues: unknown[]): unknown {
  if (node === null || typeof node !== "object") {
    return node;
  }
  if (ArrayBuffer.isView(node) || node instanceof ArrayBuffer) {
    return node;
  }
  if (Array.isArray(node)) {
    return node.map((entry) => replacePackedValues(entry, packedValues));
  }
  if (
    Object.prototype.hasOwnProperty.call(node, PACKED_PLACEHOLDER_KEY) &&
    typeof (node as { $packed?: unknown })[PACKED_PLACEHOLDER_KEY] === "number"
  ) {
    const index = (node as { $packed: number })[PACKED_PLACEHOLDER_KEY];
    const value = packedValues[index];
    if (value === undefined) {
      throw new Error(`Missing packed value at index ${index}`);
    }
    return value;
  }
  const out: Record<string, any> = {};
  for (const [key, entry] of Object.entries(node)) {
    out[key] = replacePackedValues(entry, packedValues);
  }
  return out;
}
export function encodeClientMessage(
  message: ClientMessage,
): EncodedClientMessage {
  switch (message.type) {
    case "Authenticate":
    case "ModifyQuerySet":
    case "Mutation":
    case "Action":
    case "Event": {
      return { ...message };
    }
    case "Connect": {
      if (message.maxObservedTimestamp !== undefined) {
        return {
          ...message,
          maxObservedTimestamp: longToU64(message.maxObservedTimestamp),
        };
      } else {
        return { ...message, maxObservedTimestamp: undefined };
      }
    }
    default: {
      message satisfies never;
    }
  }
  return undefined as never;
}

type U64 = Long;
type EncodedU64 = string;

/**
 * Unique nonnegative integer identifying a single query.
 */
export type QueryId = number; // nonnegative int

export type QuerySetVersion = number; // nonnegative int

export type RequestId = number; // nonnegative int

export type IdentityVersion = number; // nonnegative int

/**
 * A serialized representation of decisions made during a query's execution.
 *
 * A journal is produced when a query function first executes and is re-used
 * when a query is re-executed.
 *
 * Currently this is used to store pagination end cursors to ensure
 * that pages of paginated queries will always end at the same cursor. This
 * enables gapless, reactive pagination.
 *
 * `null` is used to represent empty journals.
 * @public
 */
export type QueryJournal = string | null;

/**
 * Client message schema
 */

type Connect = {
  type: "Connect";
  sessionId: string;
  connectionCount: number;
  lastCloseReason: string | null;
  maxObservedTimestamp?: TS | undefined;
  clientTs: number;
  supportsBinary: boolean;
};

export type AddQuery = {
  type: "Add";
  queryId: QueryId;
  udfPath: string;
  args: JSONValue[];
  journal?: QueryJournal | undefined;
  /**
   * @internal
   */
  componentPath?: string | undefined;
};

export type RemoveQuery = {
  type: "Remove";
  queryId: QueryId;
};

export type QuerySetModification = {
  type: "ModifyQuerySet";
  baseVersion: QuerySetVersion;
  newVersion: QuerySetVersion;
  modifications: (AddQuery | RemoveQuery)[];
};

export type MutationRequest = {
  type: "Mutation";
  requestId: RequestId;
  udfPath: string;
  args: JSONValue[];
  // Execute the mutation on a specific component.
  // Only admin auth is allowed to run mutations on non-root components.
  componentPath?: string | undefined;
};

export type ActionRequest = {
  type: "Action";
  requestId: RequestId;
  udfPath: string;
  args: JSONValue[];
  // Execute the action on a specific component.
  // Only admin auth is allowed to run actions on non-root components.
  componentPath?: string | undefined;
};

export type AdminAuthentication = {
  type: "Authenticate";
  tokenType: "Admin";
  value: string;
  baseVersion: IdentityVersion;
  impersonating?: UserIdentityAttributes | undefined;
};

export type Authenticate =
  | AdminAuthentication
  | {
      type: "Authenticate";
      tokenType: "User";
      value: string;
      baseVersion: IdentityVersion;
    }
  | {
      type: "Authenticate";
      tokenType: "None";
      baseVersion: IdentityVersion;
    };

export type Event = {
  type: "Event";
  eventType: string;
  event: any;
};
export type ClientMessage =
  | Connect
  | Authenticate
  | QuerySetModification
  | MutationRequest
  | ActionRequest
  | Event;

type EncodedConnect = Omit<Connect, "maxObservedTimestamp"> & {
  maxObservedTimestamp?: EncodedTS | undefined;
};

// It's not a big deal to add `| undefined` to any optional properties here because
// these messages are bound for JSON.stringify() serialization, which removes properties
// that are undefined.
type EncodedClientMessage =
  | EncodedConnect
  | Authenticate
  | QuerySetModification
  | MutationRequest
  | ActionRequest
  | Event;

/**
 * Server message schema
 */
export type TS = U64;
type EncodedTS = EncodedU64;
type LogLines = string[];

export type StateVersion = {
  querySet: QuerySetVersion;
  ts: TS;
  identity: IdentityVersion;
};
type EncodedStateVersion = Omit<StateVersion, "ts"> & { ts: EncodedTS };

type StateModification =
  | {
      type: "QueryUpdated";
      queryId: QueryId;
      value: JSONValue;
      logLines: LogLines;
      journal: QueryJournal;
    }
  | {
      type: "QueryFailed";
      queryId: QueryId;
      errorMessage: string;
      logLines: LogLines;
      errorData: JSONValue;
      journal: QueryJournal;
    }
  | {
      type: "QueryRemoved";
      queryId: QueryId;
    };

export type Transition = {
  type: "Transition";
  startVersion: StateVersion;
  endVersion: StateVersion;
  modifications: StateModification[];
  clientClockSkew?: number;
  serverTs?: number;
};

export type TransitionChunk = {
  type: "TransitionChunk";
  chunk: string;
  partNumber: number;
  totalParts: number;
  transitionId: string;
};

type MutationSuccess = {
  type: "MutationResponse";
  requestId: RequestId;
  success: true;
  result: JSONValue;
  ts: TS;
  logLines: LogLines;
};
type MutationFailed = {
  type: "MutationResponse";
  requestId: RequestId;
  success: false;
  result: string;
  logLines: LogLines;
  errorData?: JSONValue;
};
export type MutationResponse = MutationSuccess | MutationFailed;
type ActionSuccess = {
  type: "ActionResponse";
  requestId: RequestId;
  success: true;
  result: JSONValue;
  logLines: LogLines;
};
type ActionFailed = {
  type: "ActionResponse";
  requestId: RequestId;
  success: false;
  result: string;
  logLines: LogLines;
  errorData?: JSONValue;
};
export type ActionResponse = ActionSuccess | ActionFailed;
export type AuthError = {
  type: "AuthError";
  error: string;
  baseVersion: IdentityVersion;
  // True if this error is in response to processing a new `Authenticate` message.
  // Other AuthErrors may occur due to executing a function with expired auth and
  // should be handled differently.
  authUpdateAttempted: boolean;
};
type FatalError = {
  type: "FatalError";
  error: string;
};
type Ping = {
  type: "Ping";
};

// Server Messages without the messages only visible to WebSocketManager
export type ServerMessage =
  | Transition
  | MutationResponse
  | ActionResponse
  | FatalError
  | AuthError;

export type WireServerMessage =
  | Transition
  | TransitionChunk
  | MutationResponse
  | ActionResponse
  | FatalError
  | AuthError
  | Ping;

type EncodedTransition = Omit<Transition, "startVersion" | "endVersion"> & {
  startVersion: EncodedStateVersion;
  endVersion: EncodedStateVersion;
};
type EncodedMutationSuccess = Omit<MutationSuccess, "ts"> & { ts: EncodedTS };
type EncodedMutationResponse = MutationFailed | EncodedMutationSuccess;

type EncodedServerMessage =
  | EncodedTransition
  | TransitionChunk
  | EncodedMutationResponse
  | ActionResponse
  | FatalError
  | AuthError
  | Ping;
