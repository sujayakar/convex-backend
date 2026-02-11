/**
 * SDK Client Simulation - ConvexClient wrapper for DST framework.
 *
 * This module is SELF-CONTAINED: it does NOT import from ./websocket or
 * ./protocol to avoid their module-level side effects (which create a
 * second BaseConvexClient). Instead, it inlines the TestingWebSocket
 * class and the outgoing/incoming message protocol.
 *
 * This exercises the real ConvexClient (the higher-level JS SDK client):
 * - Callback-based subscription model (onUpdate)
 * - Promise-based mutation/action execution
 * - setTimeout scheduling for deferred callbacks
 * - Connection state tracking
 */

import { ConvexClient } from "convex/browser";
import { convexToJson } from "convex/values";

// ============================================================
// Inlined WebSocket shim (from websocket.ts, without side effects)
// ============================================================

type OutgoingMessage =
  | { type: "connect"; webSocketId: number }
  | { type: "send"; webSocketId: number; data: string }
  | { type: "close"; webSocketId: number }
  // Persistence messages (not used by ConvexClient but part of the protocol)
  | { type: "persistMutation"; persistId: string; mutationInfo: any }
  | { type: "persistPages"; persistId: string; pages: any[] }
  | {
      type: "mutationDone";
      mutationId: number;
      result: { type: "success"; value: any } | { type: "failure"; error: string };
    };

const outgoingMessages: OutgoingMessage[] = [];

type IncomingMessage =
  | { type: "connected"; webSocketId: number }
  | { type: "message"; webSocketId: number; data: string }
  | { type: "closed"; webSocketId: number }
  | { type: "persistenceDone"; persistId: string; error?: string };

let nextWebSocketId = 0;
const webSockets = new Map<number, SdkTestingWebSocket>();

class SdkTestingWebSocket {
  websocketId: number;

  onopen?: (this: SdkTestingWebSocket, ev: Event) => any;
  onerror?: (this: SdkTestingWebSocket, ev: Event) => any;
  onmessage?: (this: SdkTestingWebSocket, ev: MessageEvent) => any;
  onclose?: (this: SdkTestingWebSocket, ev: CloseEvent) => any;

  constructor(_url: string | URL, _protocols?: string | string[]) {
    this.websocketId = nextWebSocketId++;
    webSockets.set(this.websocketId, this);
    outgoingMessages.push({ type: "connect", webSocketId: this.websocketId });
  }

  send(data: string | ArrayBuffer | Blob | ArrayBufferView) {
    if (typeof data !== "string") {
      throw new Error("Only strings are supported");
    }
    outgoingMessages.push({
      type: "send",
      webSocketId: this.websocketId,
      data,
    });
  }

  close() {
    outgoingMessages.push({ type: "close", webSocketId: this.websocketId });
  }
}

// ============================================================
// Protocol functions (inlined from protocol.ts)
// ============================================================

export function getOutgoingMessages() {
  const result = [...outgoingMessages];
  outgoingMessages.length = 0;
  return result;
}

export function receiveIncomingMessages(messages: IncomingMessage[]) {
  for (const message of messages) {
    switch (message.type) {
      case "connected": {
        const ws = webSockets.get(message.webSocketId);
        if (!ws) {
          throw new Error(`Unknown websocket id: ${message.webSocketId}`);
        }
        if (ws.onopen) {
          ws.onopen(new Event("open"));
        }
        break;
      }
      case "message": {
        const ws = webSockets.get(message.webSocketId);
        if (!ws) {
          throw new Error(`Unknown websocket id: ${message.webSocketId}`);
        }
        if (ws.onmessage) {
          ws.onmessage({ data: message.data } as any);
        }
        break;
      }
      case "closed": {
        const ws = webSockets.get(message.webSocketId);
        if (!ws) {
          throw new Error(`Unknown websocket id: ${message.webSocketId}`);
        }
        if (ws.onclose) {
          ws.onclose({ code: 1000 } as any);
        }
        webSockets.delete(message.webSocketId);
        break;
      }
      case "persistenceDone": {
        // No-op for ConvexClient (no local persistence)
        break;
      }
      default: {
        message satisfies never;
      }
    }
  }
}

// ============================================================
// ConvexClient instance
// ============================================================

const address = "https://suadero.example.com";
const sdkClient = new ConvexClient(address, {
  unsavedChangesWarning: false,
  skipConvexDeploymentUrlCheck: true,
  webSocketConstructor: SdkTestingWebSocket as any,
});

// ============================================================
// Subscription API
// ============================================================

interface SubscriptionState {
  value?: unknown;
  error?: string;
  hasReceived: boolean;
  unsubscribe: () => void;
}

const subscriptions = new Map<number, SubscriptionState>();
let nextSubscriptionId = 0;

/**
 * Subscribe to a query using ConvexClient.onUpdate.
 * Returns a numeric subscription ID.
 */
export function subscribe(args: {
  queryName: string;
  argsJson: string;
}): number {
  const id = nextSubscriptionId++;
  const queryArgs = JSON.parse(args.argsJson);

  const state: SubscriptionState = {
    hasReceived: false,
    unsubscribe: () => {},
  };

  const unsub = sdkClient.onUpdate(
    args.queryName as any,
    queryArgs,
    (result: unknown) => {
      state.value = result;
      state.error = undefined;
      state.hasReceived = true;
    },
    (error: Error) => {
      state.error = error.message;
      state.value = undefined;
      state.hasReceived = true;
    },
  );

  state.unsubscribe = unsub;
  subscriptions.set(id, state);
  return id;
}

/**
 * Get the latest result for a subscription.
 * Returns null if no result has been received yet.
 */
export function getSubscriptionResult(subscriptionId: number): string | null {
  const state = subscriptions.get(subscriptionId);
  if (!state || !state.hasReceived) {
    return null;
  }
  if (state.error !== undefined) {
    return JSON.stringify({ error: state.error });
  }
  return JSON.stringify({ value: convexToJson(state.value as any) });
}

/**
 * Unsubscribe from a query.
 */
export function unsubscribe(subscriptionId: number): void {
  const state = subscriptions.get(subscriptionId);
  if (state) {
    state.unsubscribe();
    subscriptions.delete(subscriptionId);
  }
}

// ============================================================
// Mutation API
// ============================================================

interface MutationResult {
  type: "success" | "failure";
  value?: unknown;
  error?: string;
}

const completedMutations: Array<{
  mutationId: number;
  result: MutationResult;
}> = [];

/**
 * Run a mutation via ConvexClient.mutation (Promise-based).
 */
export function runMutation(args: {
  mutationId: number;
  name: string;
  argsJson: string;
}): void {
  const mutationArgs = JSON.parse(args.argsJson);

  sdkClient
    .mutation(args.name as any, mutationArgs)
    .then((result: unknown) => {
      completedMutations.push({
        mutationId: args.mutationId,
        result: {
          type: "success",
          value: convexToJson(result as any),
        },
      });
    })
    .catch((error: Error) => {
      completedMutations.push({
        mutationId: args.mutationId,
        result: {
          type: "failure",
          error: error.toString(),
        },
      });
    });
}

/**
 * Drain completed mutation results.
 */
export function getCompletedMutations(): Array<{
  mutationId: number;
  result: MutationResult;
}> {
  const results = [...completedMutations];
  completedMutations.length = 0;
  return results;
}

// ============================================================
// Utility
// ============================================================

/**
 * Get the max observed timestamp from the underlying BaseConvexClient.
 */
export function getSdkMaxObservedTimestamp(): string | undefined {
  return sdkClient.client.getMaxObservedTimestamp()?.toString();
}
