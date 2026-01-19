import * as Base64 from "base64-js";

export const packedValueSymbol = Symbol.for("convex.packed");

function isPackedValue(value: unknown): boolean {
  return typeof value === "object" && value !== null && (value as any)[packedValueSymbol];
}

function base64ToBigInt(encoded: string): bigint {
  const integerBytes = Base64.toByteArray(encoded);
  if (integerBytes.byteLength !== 8) {
    throw new Error(
      `Received ${integerBytes.byteLength} bytes, expected 8 for $integer`,
    );
  }
  const intBytesView = new DataView(integerBytes.buffer);
  return intBytesView.getBigInt64(0, true);
}

function base64ToBytes(encoded: string): ArrayBuffer {
  const bytes = Base64.toByteArray(encoded);
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
}

function parseSpecialFloat(encoded: string): number {
  switch (encoded) {
    case "NaN":
      return Number.NaN;
    case "Infinity":
      return Number.POSITIVE_INFINITY;
    case "-Infinity":
      return Number.NEGATIVE_INFINITY;
    case "-0":
      return -0;
    default:
      throw new Error(`Unknown $float value ${encoded}`);
  }
}

export function jsonToConvex(value: any): any {
  if (isPackedValue(value)) {
    return value;
  }
  if (value === null) {
    return value;
  }
  if (typeof value === "boolean" || typeof value === "number" || typeof value === "string") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map(jsonToConvex);
  }
  if (typeof value !== "object") {
    return value;
  }
  if ("$integer" in value && typeof value.$integer === "string") {
    return base64ToBigInt(value.$integer);
  }
  if ("$bytes" in value && typeof value.$bytes === "string") {
    return base64ToBytes(value.$bytes);
  }
  if ("$float" in value && typeof value.$float === "string") {
    return parseSpecialFloat(value.$float);
  }
  const out: Record<string, any> = {};
  for (const [key, entry] of Object.entries(value)) {
    out[key] = jsonToConvex(entry);
  }
  return out;
}
