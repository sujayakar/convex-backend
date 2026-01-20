import { performOp } from "udf-syscall-ffi";

import {
  jsonToConvex,
  packedValueSymbol,
} from "./values.js";

type PackedValueKind = "object" | "array";
type PackedPath = Array<string | number>;

interface PackedMeta {
  handle: number;
  kind: PackedValueKind;
  path: PackedPath;
  materialized?: any;
}

function isArrayIndex(prop: string): boolean {
  if (prop === "") return false;
  const n = Number(prop);
  return Number.isInteger(n) && n >= 0 && String(n) === prop;
}

function unpackReadResult(handle: number, result: any): any {
  switch (result?.kind) {
    case "missing":
      return undefined;
    case "packed":
      return createPackedValue(handle, result.valueKind, result.path);
    case "value":
      return jsonToConvex(result.value);
    default:
      return undefined;
  }
}

function materializePackedValue(meta: PackedMeta): any {
  if (meta.kind === "array") {
    const length = performOp("packedValue/length", meta.handle, meta.path);
    const out = new Array(length);
    for (let i = 0; i < length; i++) {
      const result = performOp("packedValue/read", meta.handle, meta.path, i);
      out[i] = unpackReadResult(meta.handle, result);
    }
    return out;
  }
  const keys = performOp("packedValue/keys", meta.handle, meta.path);
  const out: Record<string, any> = {};
  for (const key of keys) {
    const result = performOp("packedValue/read", meta.handle, meta.path, key);
    out[key] = unpackReadResult(meta.handle, result);
  }
  return out;
}

function getMaterialized(meta: PackedMeta): any {
  if (meta.materialized === undefined) {
    meta.materialized = materializePackedValue(meta);
  }
  return meta.materialized;
}

const packedValueHandler: ProxyHandler<any> = {
  get(target, prop, receiver) {
    const meta: PackedMeta = target[packedValueSymbol];
    if (prop === packedValueSymbol) {
      return meta;
    }
    if (meta.materialized !== undefined) {
      return Reflect.get(meta.materialized, prop, receiver);
    }
    if (prop === Symbol.toStringTag) {
      return meta.kind === "array" ? "Array" : "Object";
    }
    if (prop === Symbol.iterator && meta.kind === "array") {
      return function* () {
        const length = performOp("packedValue/length", meta.handle, meta.path);
        for (let i = 0; i < length; i++) {
          yield receiver[i];
        }
      };
    }
    if (prop === "length" && meta.kind === "array") {
      return performOp("packedValue/length", meta.handle, meta.path);
    }
    if (typeof prop !== "string") {
      return undefined;
    }
    const key =
      meta.kind === "array" && isArrayIndex(prop) ? Number(prop) : prop;
    const result = performOp("packedValue/read", meta.handle, meta.path, key);
    return unpackReadResult(meta.handle, result);
  },
  has(target, prop) {
    const meta: PackedMeta = target[packedValueSymbol];
    if (prop === packedValueSymbol) {
      return true;
    }
    if (meta.materialized !== undefined) {
      return Reflect.has(meta.materialized, prop);
    }
    if (typeof prop !== "string") {
      return false;
    }
    if (prop === "length" && meta.kind === "array") {
      return true;
    }
    const key =
      meta.kind === "array" && isArrayIndex(prop) ? Number(prop) : prop;
    return performOp("packedValue/has", meta.handle, meta.path, key);
  },
  ownKeys(target) {
    const meta: PackedMeta = target[packedValueSymbol];
    if (meta.materialized !== undefined) {
      const keys = Reflect.ownKeys(meta.materialized);
      keys.push(packedValueSymbol);
      return keys;
    }
    const keys = performOp("packedValue/keys", meta.handle, meta.path);
    if (meta.kind === "array") {
      keys.push("length");
    }
    keys.push(packedValueSymbol);
    return keys;
  },
  getOwnPropertyDescriptor(target, prop) {
    const meta: PackedMeta = target[packedValueSymbol];
    if (prop === packedValueSymbol) {
      return Object.getOwnPropertyDescriptor(target, prop);
    }
    if (meta.materialized !== undefined) {
      return Object.getOwnPropertyDescriptor(meta.materialized, prop);
    }
    if (typeof prop !== "string") {
      return undefined;
    }
    if (prop === "length" && meta.kind === "array") {
      return {
        configurable: false,
        enumerable: false,
        value: performOp("packedValue/length", meta.handle, meta.path),
        writable: false,
      };
    }
    const key =
      meta.kind === "array" && isArrayIndex(prop) ? Number(prop) : prop;
    if (!performOp("packedValue/has", meta.handle, meta.path, key)) {
      return undefined;
    }
    const readResult = performOp("packedValue/read", meta.handle, meta.path, key);
    return {
      configurable: true,
      enumerable: true,
      value: unpackReadResult(meta.handle, readResult),
    };
  },
  set(target, prop, value, receiver) {
    const meta: PackedMeta = target[packedValueSymbol];
    const materialized = getMaterialized(meta);
    return Reflect.set(materialized, prop, value, receiver);
  },
  deleteProperty(target, prop) {
    const meta: PackedMeta = target[packedValueSymbol];
    const materialized = getMaterialized(meta);
    return Reflect.deleteProperty(materialized, prop);
  },
  defineProperty(target, prop, descriptor) {
    const meta: PackedMeta = target[packedValueSymbol];
    const materialized = getMaterialized(meta);
    Object.defineProperty(materialized, prop, descriptor);
    return true;
  },
};

function createPackedValue(
  handle: number,
  kind: PackedValueKind,
  path: PackedPath = [],
): any {
  const target: Record<string, any> = {};
  const meta: PackedMeta = { handle, kind, path };
  Object.defineProperty(target, packedValueSymbol, {
    value: meta,
    enumerable: false,
  });
  return new Proxy(target, packedValueHandler);
}

export function setupPackedValues(global: any) {
  const internal = Object.create(null);
  Object.defineProperty(global, "__convexInternal", {
    value: internal,
    enumerable: false,
  });
  internal.createPackedValue = (
    handle: number,
    kind: PackedValueKind,
    path: PackedPath = [],
  ) => createPackedValue(handle, kind, path);
}
