/**
 * @vitest-environment custom-vitest-environment.ts
 */

import { test, expect } from "vitest";

import * as flexbuffers from "flatbuffers/js/flexbuffers.js";
import { Long } from "../../vendor/long.js";
import {
  decodeBinaryFrame,
  longToU64,
  parseBinaryServerMessage,
  u64ToLong,
} from "./protocol.js";

test("Long serialization", async () => {
  expect(Long.fromNumber(89234097497)).toEqual(
    u64ToLong(longToU64(Long.fromNumber(89234097497))),
  );
});

test("Binary server message parsing", async () => {
  const packed = flexbuffers.encode({ foo: 42, bar: "baz" });
  const header = {
    type: "MutationResponse",
    requestId: 1,
    success: true,
    result: { $packed: 0 },
    ts: longToU64(Long.fromNumber(123)),
    logLines: [],
  };
  const headerBytes = new TextEncoder().encode(JSON.stringify(header));
  const payload = new Uint8Array(4 + headerBytes.length + 4 + packed.length);
  const view = new DataView(payload.buffer);
  view.setUint32(0, headerBytes.length, true);
  payload.set(headerBytes, 4);
  let offset = 4 + headerBytes.length;
  view.setUint32(offset, packed.length, true);
  offset += 4;
  payload.set(packed, offset);

  const message = parseBinaryServerMessage(payload);
  expect(message.type).toBe("MutationResponse");
  if (message.type === "MutationResponse" && message.success) {
    expect(message.result).toEqual({ bar: "baz", foo: 42 });
    expect(message.ts).toEqual(Long.fromNumber(123));
  } else {
    throw new Error("Unexpected server message");
  }
});

test("Binary frame decoding", async () => {
  const payload = new Uint8Array([1, 2, 3]);
  const fullFrame = new Uint8Array(1 + payload.length);
  fullFrame[0] = 0;
  fullFrame.set(payload, 1);
  expect(decodeBinaryFrame(fullFrame.buffer)).toEqual({
    type: "full",
    payload,
  });

  const chunkPayload = new Uint8Array([9, 8]);
  const chunkFrame = new Uint8Array(13 + chunkPayload.length);
  const view = new DataView(chunkFrame.buffer);
  chunkFrame[0] = 1;
  view.setUint32(1, 7, true);
  view.setUint32(5, 2, true);
  view.setUint32(9, 4, true);
  chunkFrame.set(chunkPayload, 13);
  expect(decodeBinaryFrame(chunkFrame.buffer)).toEqual({
    type: "chunk",
    messageId: 7,
    partNumber: 2,
    totalParts: 4,
    payload: chunkPayload,
  });
});
