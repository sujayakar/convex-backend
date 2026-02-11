import {
  a as n
} from "./U5LILI2V.js";

// ../convex/dist/esm/values/base64.js
var S = [], b = [], Ut = Uint8Array, Ae = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
for (T = 0, et = Ae.length; T < et; ++T)
  S[T] = Ae[T], b[Ae.charCodeAt(T)] = T;
var T, et;
b[45] = 62;
b[95] = 63;
function Jt(e) {
  var t = e.length;
  if (t % 4 > 0)
    throw new Error("Invalid string. Length must be a multiple of 4");
  var r = e.indexOf("=");
  r === -1 && (r = t);
  var o = r === t ? 0 : 4 - r % 4;
  return [r, o];
}
n(Jt, "getLens");
function Vt(e, t, r) {
  return (t + r) * 3 / 4 - r;
}
n(Vt, "_byteLength");
function U(e) {
  var t, r = Jt(e), o = r[0], s = r[1], i = new Ut(Vt(e, o, s)), a = 0, u = s > 0 ? o - 4 : o, p;
  for (p = 0; p < u; p += 4)
    t = b[e.charCodeAt(p)] << 18 | b[e.charCodeAt(p + 1)] << 12 | b[e.charCodeAt(p + 2)] << 6 | b[e.charCodeAt(p + 3)], i[a++] = t >> 16 & 255, i[a++] = t >> 8 & 255, i[a++] = t & 255;
  return s === 2 && (t = b[e.charCodeAt(p)] << 2 | b[e.charCodeAt(p + 1)] >> 4, i[a++] = t & 255), s === 1 && (t = b[e.charCodeAt(p)] << 10 | b[e.charCodeAt(p + 1)] << 4 | b[e.charCodeAt(p + 2)] >> 2, i[a++] = t >> 8 & 255, i[a++] = t & 255), i;
}
n(U, "toByteArray");
function Lt(e) {
  return S[e >> 18 & 63] + S[e >> 12 & 63] + S[e >> 6 & 63] + S[e & 63];
}
n(Lt, "tripletToBase64");
function kt(e, t, r) {
  for (var o, s = [], i = t; i < r; i += 3)
    o = (e[i] << 16 & 16711680) + (e[i + 1] << 8 & 65280) + (e[i + 2] & 255), s.push(Lt(o));
  return s.join("");
}
n(kt, "encodeChunk");
function J(e) {
  for (var t, r = e.length, o = r % 3, s = [], i = 16383, a = 0, u = r - o; a < u; a += i)
    s.push(
      kt(
        e,
        a,
        a + i > u ? u : a + i
      )
    );
  return o === 1 ? (t = e[r - 1], s.push(S[t >> 2] + S[t << 4 & 63] + "==")) : o === 2 && (t = (e[r - 2] << 8) + e[r - 1], s.push(
    S[t >> 10] + S[t >> 4 & 63] + S[t << 2 & 63] + "="
  )), s.join("");
}
n(J, "fromByteArray");

// ../convex/dist/esm/common/index.js
function O(e) {
  if (e === void 0)
    return {};
  if (!Ee(e))
    throw new Error(
      `The arguments to a Convex function must be an object. Received: ${e}`
    );
  return e;
}
n(O, "parseArgs");
function Ee(e) {
  let t = typeof e == "object", r = Object.getPrototypeOf(e), o = r === null || r === Object.prototype || // Objects generated from other contexts (e.g. across Node.js `vm` modules) will not satisfy the previous
  // conditions but are still simple objects.
  r?.constructor?.name === "Object";
  return t && o;
}
n(Ee, "isSimpleObject");

// ../convex/dist/esm/values/value.js
var ot = !0, R = BigInt("-9223372036854775808"), Te = BigInt("9223372036854775807"), Se = BigInt("0"), Dt = BigInt("8"), Qt = BigInt("256");
function st(e) {
  return Number.isNaN(e) || !Number.isFinite(e) || Object.is(e, -0);
}
n(st, "isSpecial");
function Ht(e) {
  e < Se && (e -= R + R);
  let t = e.toString(16);
  t.length % 2 === 1 && (t = "0" + t);
  let r = new Uint8Array(new ArrayBuffer(8)), o = 0;
  for (let s of t.match(/.{2}/g).reverse())
    r.set([parseInt(s, 16)], o++), e >>= Dt;
  return J(r);
}
n(Ht, "slowBigIntToBase64");
function Gt(e) {
  let t = U(e);
  if (t.byteLength !== 8)
    throw new Error(
      `Received ${t.byteLength} bytes, expected 8 for $integer`
    );
  let r = Se, o = Se;
  for (let s of t)
    r += BigInt(s) * Qt ** o, o++;
  return r > Te && (r += R + R), r;
}
n(Gt, "slowBase64ToBigInt");
function Wt(e) {
  if (e < R || Te < e)
    throw new Error(
      `BigInt ${e} does not fit into a 64-bit signed integer.`
    );
  let t = new ArrayBuffer(8);
  return new DataView(t).setBigInt64(0, e, !0), J(new Uint8Array(t));
}
n(Wt, "modernBigIntToBase64");
function zt(e) {
  let t = U(e);
  if (t.byteLength !== 8)
    throw new Error(
      `Received ${t.byteLength} bytes, expected 8 for $integer`
    );
  return new DataView(t.buffer).getBigInt64(0, !0);
}
n(zt, "modernBase64ToBigInt");
var Xt = DataView.prototype.setBigInt64 ? Wt : Ht, Yt = DataView.prototype.getBigInt64 ? zt : Gt, rt = 1024;
function Oe(e) {
  if (e.length > rt)
    throw new Error(
      `Field name ${e} exceeds maximum field name length ${rt}.`
    );
  if (e.startsWith("$"))
    throw new Error(`Field name ${e} starts with a '$', which is reserved.`);
  for (let t = 0; t < e.length; t += 1) {
    let r = e.charCodeAt(t);
    if (r < 32 || r >= 127)
      throw new Error(
        `Field name ${e} has invalid character '${e[t]}': Field names can only contain non-control ASCII characters`
      );
  }
}
n(Oe, "validateObjectField");
function y(e) {
  if (e === null || typeof e == "boolean" || typeof e == "number" || typeof e == "string")
    return e;
  if (Array.isArray(e))
    return e.map((o) => y(o));
  if (typeof e != "object")
    throw new Error(`Unexpected type of ${e}`);
  let t = Object.entries(e);
  if (t.length === 1) {
    let o = t[0][0];
    if (o === "$bytes") {
      if (typeof e.$bytes != "string")
        throw new Error(`Malformed $bytes field on ${e}`);
      return U(e.$bytes).buffer;
    }
    if (o === "$integer") {
      if (typeof e.$integer != "string")
        throw new Error(`Malformed $integer field on ${e}`);
      return Yt(e.$integer);
    }
    if (o === "$float") {
      if (typeof e.$float != "string")
        throw new Error(`Malformed $float field on ${e}`);
      let s = U(e.$float);
      if (s.byteLength !== 8)
        throw new Error(
          `Received ${s.byteLength} bytes, expected 8 for $float`
        );
      let a = new DataView(s.buffer).getFloat64(0, ot);
      if (!st(a))
        throw new Error(`Float ${a} should be encoded as a number`);
      return a;
    }
    if (o === "$set")
      throw new Error(
        "Received a Set which is no longer supported as a Convex type."
      );
    if (o === "$map")
      throw new Error(
        "Received a Map which is no longer supported as a Convex type."
      );
  }
  let r = {};
  for (let [o, s] of Object.entries(e))
    Oe(o), r[o] = y(s);
  return r;
}
n(y, "jsonToConvex");
var nt = 16384;
function _(e) {
  let t = JSON.stringify(e, (r, o) => o === void 0 ? "undefined" : typeof o == "bigint" ? `${o.toString()}n` : o);
  if (t.length > nt) {
    let r = "[...truncated]", o = nt - r.length, s = t.codePointAt(o - 1);
    return s !== void 0 && s > 65535 && (o -= 1), t.substring(0, o) + r;
  }
  return t;
}
n(_, "stringifyValueForError");
function V(e, t, r, o) {
  if (e === void 0) {
    let a = r && ` (present at path ${r} in original object ${_(
      t
    )})`;
    throw new Error(
      `undefined is not a valid Convex value${a}. To learn about Convex's supported types, see https://docs.convex.dev/using/types.`
    );
  }
  if (e === null)
    return e;
  if (typeof e == "bigint") {
    if (e < R || Te < e)
      throw new Error(
        `BigInt ${e} does not fit into a 64-bit signed integer.`
      );
    return { $integer: Xt(e) };
  }
  if (typeof e == "number")
    if (st(e)) {
      let a = new ArrayBuffer(8);
      return new DataView(a).setFloat64(0, e, ot), { $float: J(new Uint8Array(a)) };
    } else
      return e;
  if (typeof e == "boolean" || typeof e == "string")
    return e;
  if (e instanceof ArrayBuffer)
    return { $bytes: J(new Uint8Array(e)) };
  if (Array.isArray(e))
    return e.map(
      (a, u) => V(a, t, r + `[${u}]`, !1)
    );
  if (e instanceof Set)
    throw new Error(
      Ie(r, "Set", [...e], t)
    );
  if (e instanceof Map)
    throw new Error(
      Ie(r, "Map", [...e], t)
    );
  if (!Ee(e)) {
    let a = e?.constructor?.name, u = a ? `${a} ` : "";
    throw new Error(
      Ie(r, u, e, t)
    );
  }
  let s = {}, i = Object.entries(e);
  i.sort(([a, u], [p, I]) => a === p ? 0 : a < p ? -1 : 1);
  for (let [a, u] of i)
    u !== void 0 ? (Oe(a), s[a] = V(u, t, r + `.${a}`, !1)) : o && (Oe(a), s[a] = it(
      u,
      t,
      r + `.${a}`
    ));
  return s;
}
n(V, "convexToJsonInternal");
function Ie(e, t, r, o) {
  return e ? `${t}${_(
    r
  )} is not a supported Convex type (present at path ${e} in original object ${_(
    o
  )}). To learn about Convex's supported types, see https://docs.convex.dev/using/types.` : `${t}${_(
    r
  )} is not a supported Convex type.`;
}
n(Ie, "errorMessageForUnsupportedType");
function it(e, t, r) {
  if (e === void 0)
    return { $undefined: null };
  if (t === void 0)
    throw new Error(
      `Programming error. Current value is ${_(
        e
      )} but original value is undefined`
    );
  return V(e, t, r, !1);
}
n(it, "convexOrUndefinedToJsonInternal");
function h(e) {
  return V(e, e, "", !1);
}
n(h, "convexToJson");
function v(e) {
  return it(e, e, "");
}
n(v, "convexOrUndefinedToJson");
function at(e) {
  return V(e, e, "", !0);
}
n(at, "patchValueToJson");

// ../convex/dist/esm/values/validators.js
var Kt = Object.defineProperty, Zt = /* @__PURE__ */ n((e, t, r) => t in e ? Kt(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), m = /* @__PURE__ */ n((e, t, r) => Zt(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), er = "https://docs.convex.dev/error#undefined-validator";
function L(e, t) {
  let r = t !== void 0 ? ` for field "${t}"` : "";
  throw new Error(
    `A validator is undefined${r} in ${e}. This is often caused by circular imports. See ${er} for details.`
  );
}
n(L, "throwUndefinedValidatorError");
var g = class {
  static {
    n(this, "BaseValidator");
  }
  constructor({ isOptional: t }) {
    m(this, "type"), m(this, "fieldPaths"), m(this, "isOptional"), m(this, "isConvexValidator"), this.isOptional = t, this.isConvexValidator = !0;
  }
}, Y = class e extends g {
  static {
    n(this, "VId");
  }
  /**
   * Usually you'd use `v.id(tableName)` instead.
   */
  constructor({
    isOptional: t,
    tableName: r
  }) {
    if (super({ isOptional: t }), m(this, "tableName"), m(this, "kind", "id"), typeof r != "string")
      throw new Error("v.id(tableName) requires a string");
    this.tableName = r;
  }
  /** @internal */
  get json() {
    return { type: "id", tableName: this.tableName };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional",
      tableName: this.tableName
    });
  }
}, k = class e extends g {
  static {
    n(this, "VFloat64");
  }
  constructor() {
    super(...arguments), m(this, "kind", "float64");
  }
  /** @internal */
  get json() {
    return { type: "number" };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional"
    });
  }
}, D = class e extends g {
  static {
    n(this, "VInt64");
  }
  constructor() {
    super(...arguments), m(this, "kind", "int64");
  }
  /** @internal */
  get json() {
    return { type: "bigint" };
  }
  /** @internal */
  asOptional() {
    return new e({ isOptional: "optional" });
  }
}, K = class e extends g {
  static {
    n(this, "VBoolean");
  }
  constructor() {
    super(...arguments), m(this, "kind", "boolean");
  }
  /** @internal */
  get json() {
    return { type: this.kind };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional"
    });
  }
}, Z = class e extends g {
  static {
    n(this, "VBytes");
  }
  constructor() {
    super(...arguments), m(this, "kind", "bytes");
  }
  /** @internal */
  get json() {
    return { type: this.kind };
  }
  /** @internal */
  asOptional() {
    return new e({ isOptional: "optional" });
  }
}, ee = class e extends g {
  static {
    n(this, "VString");
  }
  constructor() {
    super(...arguments), m(this, "kind", "string");
  }
  /** @internal */
  get json() {
    return { type: this.kind };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional"
    });
  }
}, te = class e extends g {
  static {
    n(this, "VNull");
  }
  constructor() {
    super(...arguments), m(this, "kind", "null");
  }
  /** @internal */
  get json() {
    return { type: this.kind };
  }
  /** @internal */
  asOptional() {
    return new e({ isOptional: "optional" });
  }
}, re = class e extends g {
  static {
    n(this, "VAny");
  }
  constructor() {
    super(...arguments), m(this, "kind", "any");
  }
  /** @internal */
  get json() {
    return {
      type: this.kind
    };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional"
    });
  }
}, ne = class e extends g {
  static {
    n(this, "VObject");
  }
  /**
   * Usually you'd use `v.object({ ... })` instead.
   */
  constructor({
    isOptional: t,
    fields: r
  }) {
    super({ isOptional: t }), m(this, "fields"), m(this, "kind", "object"), globalThis.Object.entries(r).forEach(([o, s]) => {
      if (s === void 0 && L("v.object()", o), !s.isConvexValidator)
        throw new Error("v.object() entries must be validators");
    }), this.fields = r;
  }
  /** @internal */
  get json() {
    return {
      type: this.kind,
      value: globalThis.Object.fromEntries(
        globalThis.Object.entries(this.fields).map(([t, r]) => [
          t,
          {
            fieldType: r.json,
            optional: r.isOptional === "optional"
          }
        ])
      )
    };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional",
      fields: this.fields
    });
  }
  /**
   * Create a new VObject with the specified fields omitted.
   * @param fields The field names to omit from this VObject.
   */
  omit(...t) {
    let r = { ...this.fields };
    for (let o of t)
      delete r[o];
    return new e({
      isOptional: this.isOptional,
      fields: r
    });
  }
  /**
   * Create a new VObject with only the specified fields.
   * @param fields The field names to pick from this VObject.
   */
  pick(...t) {
    let r = {};
    for (let o of t)
      r[o] = this.fields[o];
    return new e({
      isOptional: this.isOptional,
      fields: r
    });
  }
  /**
   * Create a new VObject with all fields marked as optional.
   */
  partial() {
    let t = {};
    for (let [r, o] of globalThis.Object.entries(this.fields))
      t[r] = o.asOptional();
    return new e({
      isOptional: this.isOptional,
      fields: t
    });
  }
  /**
   * Create a new VObject with additional fields merged in.
   * @param fields An object with additional validators to merge into this VObject.
   */
  extend(t) {
    return new e({
      isOptional: this.isOptional,
      fields: { ...this.fields, ...t }
    });
  }
}, oe = class e extends g {
  static {
    n(this, "VLiteral");
  }
  /**
   * Usually you'd use `v.literal(value)` instead.
   */
  constructor({ isOptional: t, value: r }) {
    if (super({ isOptional: t }), m(this, "value"), m(this, "kind", "literal"), typeof r != "string" && typeof r != "boolean" && typeof r != "number" && typeof r != "bigint")
      throw new Error("v.literal(value) must be a string, number, or boolean");
    this.value = r;
  }
  /** @internal */
  get json() {
    return {
      type: this.kind,
      value: h(this.value)
    };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional",
      value: this.value
    });
  }
}, se = class e extends g {
  static {
    n(this, "VArray");
  }
  /**
   * Usually you'd use `v.array(element)` instead.
   */
  constructor({
    isOptional: t,
    element: r
  }) {
    super({ isOptional: t }), m(this, "element"), m(this, "kind", "array"), r === void 0 && L("v.array()"), this.element = r;
  }
  /** @internal */
  get json() {
    return {
      type: this.kind,
      value: this.element.json
    };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional",
      element: this.element
    });
  }
}, ie = class e extends g {
  static {
    n(this, "VRecord");
  }
  /**
   * Usually you'd use `v.record(key, value)` instead.
   */
  constructor({
    isOptional: t,
    key: r,
    value: o
  }) {
    if (super({ isOptional: t }), m(this, "key"), m(this, "value"), m(this, "kind", "record"), r === void 0 && L("v.record()", "key"), o === void 0 && L("v.record()", "value"), r.isOptional === "optional")
      throw new Error("Record validator cannot have optional keys");
    if (o.isOptional === "optional")
      throw new Error("Record validator cannot have optional values");
    if (!r.isConvexValidator || !o.isConvexValidator)
      throw new Error("Key and value of v.record() but be validators");
    this.key = r, this.value = o;
  }
  /** @internal */
  get json() {
    return {
      type: this.kind,
      // This cast is needed because TypeScript thinks the key type is too wide
      keys: this.key.json,
      values: {
        fieldType: this.value.json,
        optional: !1
      }
    };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional",
      key: this.key,
      value: this.value
    });
  }
}, ae = class e extends g {
  static {
    n(this, "VUnion");
  }
  /**
   * Usually you'd use `v.union(...members)` instead.
   */
  constructor({ isOptional: t, members: r }) {
    super({ isOptional: t }), m(this, "members"), m(this, "kind", "union"), r.forEach((o, s) => {
      if (o === void 0 && L("v.union()", `member at index ${s}`), !o.isConvexValidator)
        throw new Error("All members of v.union() must be validators");
    }), this.members = r;
  }
  /** @internal */
  get json() {
    return {
      type: this.kind,
      value: this.members.map((t) => t.json)
    };
  }
  /** @internal */
  asOptional() {
    return new e({
      isOptional: "optional",
      members: this.members
    });
  }
};

// ../convex/dist/esm/values/validator.js
function _e(e) {
  return !!e.isConvexValidator;
}
n(_e, "isValidator");
function ue(e) {
  return _e(e) ? e : c.object(e);
}
n(ue, "asObjectValidator");
var c = {
  /**
   * Validates that the value corresponds to an ID of a document in given table.
   * @param tableName The name of the table.
   */
  id: /* @__PURE__ */ n((e) => new Y({
    isOptional: "required",
    tableName: e
  }), "id"),
  /**
   * Validates that the value is of type Null.
   */
  null: /* @__PURE__ */ n(() => new te({ isOptional: "required" }), "null"),
  /**
   * Validates that the value is of Convex type Float64 (Number in JS).
   *
   * Alias for `v.float64()`
   */
  number: /* @__PURE__ */ n(() => new k({ isOptional: "required" }), "number"),
  /**
   * Validates that the value is of Convex type Float64 (Number in JS).
   */
  float64: /* @__PURE__ */ n(() => new k({ isOptional: "required" }), "float64"),
  /**
   * @deprecated Use `v.int64()` instead
   */
  bigint: /* @__PURE__ */ n(() => new D({ isOptional: "required" }), "bigint"),
  /**
   * Validates that the value is of Convex type Int64 (BigInt in JS).
   */
  int64: /* @__PURE__ */ n(() => new D({ isOptional: "required" }), "int64"),
  /**
   * Validates that the value is of type Boolean.
   */
  boolean: /* @__PURE__ */ n(() => new K({ isOptional: "required" }), "boolean"),
  /**
   * Validates that the value is of type String.
   */
  string: /* @__PURE__ */ n(() => new ee({ isOptional: "required" }), "string"),
  /**
   * Validates that the value is of Convex type Bytes (constructed in JS via `ArrayBuffer`).
   */
  bytes: /* @__PURE__ */ n(() => new Z({ isOptional: "required" }), "bytes"),
  /**
   * Validates that the value is equal to the given literal value.
   * @param literal The literal value to compare against.
   */
  literal: /* @__PURE__ */ n((e) => new oe({ isOptional: "required", value: e }), "literal"),
  /**
   * Validates that the value is an Array of the given element type.
   * @param element The validator for the elements of the array.
   */
  array: /* @__PURE__ */ n((e) => new se({ isOptional: "required", element: e }), "array"),
  /**
   * Validates that the value is an Object with the given properties.
   * @param fields An object specifying the validator for each property.
   */
  object: /* @__PURE__ */ n((e) => new ne({ isOptional: "required", fields: e }), "object"),
  /**
   * Validates that the value is a Record with keys and values that match the given types.
   * @param keys The validator for the keys of the record. This cannot contain string literals.
   * @param values The validator for the values of the record.
   */
  record: /* @__PURE__ */ n((e, t) => new ie({
    isOptional: "required",
    key: e,
    value: t
  }), "record"),
  /**
   * Validates that the value matches one of the given validators.
   * @param members The validators to match against.
   */
  union: /* @__PURE__ */ n((...e) => new ae({
    isOptional: "required",
    members: e
  }), "union"),
  /**
   * Does not validate the value.
   */
  any: /* @__PURE__ */ n(() => new re({ isOptional: "required" }), "any"),
  /**
   * Allows not specifying a value for a property in an Object.
   * @param value The property value validator to make optional.
   *
   * ```typescript
   * const objectWithOptionalFields = v.object({
   *   requiredField: v.string(),
   *   optionalField: v.optional(v.string()),
   * });
   * ```
   */
  optional: /* @__PURE__ */ n((e) => e.asOptional(), "optional"),
  /**
   * Allows specifying a value or null.
   */
  nullable: /* @__PURE__ */ n((e) => c.union(e, c.null()), "nullable")
};

// ../convex/dist/esm/values/errors.js
var tr = Object.defineProperty, rr = /* @__PURE__ */ n((e, t, r) => t in e ? tr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), Ce = /* @__PURE__ */ n((e, t, r) => rr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), ut, ct, nr = Symbol.for("ConvexError"), ce = class extends (ct = Error, ut = nr, ct) {
  static {
    n(this, "ConvexError");
  }
  constructor(t) {
    super(typeof t == "string" ? t : _(t)), Ce(this, "name", "ConvexError"), Ce(this, "data"), Ce(this, ut, !0), this.data = t;
  }
};

// ../convex/dist/esm/values/compare_utf8.js
function dt(e, t) {
  let r = e.length, o = t.length, s = Math.min(r, o);
  for (let i = 0; i < s; ) {
    let a = e.codePointAt(i), u = t.codePointAt(i);
    if (a !== u) {
      if (a < 128 && u < 128)
        return a - u;
      let p = pt(a, lt), I = pt(u, ft);
      return or(lt, p, ft, I);
    }
    i += sr(a);
  }
  return r - o;
}
n(dt, "compareUTF8");
function or(e, t, r, o) {
  let s = Math.min(t, o);
  for (let i = 0; i < s; i++) {
    let a = e[i], u = r[i];
    if (a !== u)
      return a - u;
  }
  return t - o;
}
n(or, "compareArrays");
function sr(e) {
  return e > 65535 ? 2 : 1;
}
n(sr, "utf16LengthForCodePoint");
var ht = /* @__PURE__ */ n(() => Array.from({ length: 4 }, () => 0), "arr"), lt = ht(), ft = ht();
function pt(e, t) {
  if (e < 128)
    return t[0] = e, 1;
  let r, o;
  if (e <= 2047)
    r = 1, o = 192;
  else if (e <= 65535)
    r = 2, o = 224;
  else if (e <= 1114111)
    r = 3, o = 240;
  else
    throw new Error("Invalid code point");
  t[0] = (e >> 6 * r) + o;
  let s = 1;
  for (; r > 0; r--) {
    let i = e >> 6 * (r - 1);
    t[s++] = 128 | i & 63;
  }
  return s;
}
n(pt, "utf8Bytes");

// ../convex/dist/esm/values/compare.js
function ir(e, t) {
  return mt(Q(e), Q(t));
}
n(ir, "compareValues");
function mt(e, t) {
  return e[0] === t[0] ? ar(e[1], t[1]) : e[0] < t[0] ? -1 : 1;
}
n(mt, "compareAsTuples");
function ar(e, t) {
  if (e == null)
    return 0;
  if (typeof e == "number") {
    if (typeof t != "number")
      throw new Error(`Unexpected type ${t}`);
    return ur(e, t);
  }
  if (typeof e == "string") {
    if (typeof t != "string")
      throw new Error(`Unexpected type ${t}`);
    return dt(e, t);
  }
  if (typeof e == "bigint" || typeof e == "boolean" || typeof e == "string")
    return e < t ? -1 : e === t ? 0 : 1;
  if (!Array.isArray(e) || !Array.isArray(t))
    throw new Error(`Unexpected type ${e}`);
  for (let r = 0; r < e.length && r < t.length; r++) {
    let o = mt(e[r], t[r]);
    if (o !== 0)
      return o;
  }
  return e.length < t.length ? -1 : e.length > t.length ? 1 : 0;
}
n(ar, "compareSameTypeValues");
function ur(e, t) {
  if (isNaN(e) || isNaN(t)) {
    let r = new ArrayBuffer(8), o = new ArrayBuffer(8);
    new DataView(r).setFloat64(
      0,
      e,
      /* little-endian */
      !0
    ), new DataView(o).setFloat64(
      0,
      t,
      /* little-endian */
      !0
    );
    let s = BigInt(
      new DataView(r).getBigInt64(
        0,
        /* little-endian */
        !0
      )
    ), i = BigInt(
      new DataView(o).getBigInt64(
        0,
        /* little-endian */
        !0
      )
    ), a = (s & 0x8000000000000000n) !== 0n, u = (i & 0x8000000000000000n) !== 0n;
    return isNaN(e) !== isNaN(t) ? isNaN(e) ? a ? -1 : 1 : u ? 1 : -1 : a !== u ? a ? -1 : 1 : s < i ? -1 : s === i ? 0 : 1;
  }
  return Object.is(e, t) ? 0 : Object.is(e, -0) ? Object.is(t, 0) ? -1 : -Math.sign(t) : Object.is(t, -0) ? Object.is(e, 0) ? 1 : Math.sign(e) : e < t ? -1 : 1;
}
n(ur, "compareNumbers");
function Q(e) {
  return e === void 0 ? [0, void 0] : e === null ? [1, null] : typeof e == "bigint" ? [2, e] : typeof e == "number" ? [3, e] : typeof e == "boolean" ? [4, e] : typeof e == "string" ? [5, e] : e instanceof ArrayBuffer ? [6, Array.from(new Uint8Array(e)).map(Q)] : Array.isArray(e) ? [7, e.map(Q)] : [8, Object.keys(e).sort().map((o) => [o, e[o]]).map(Q)];
}
n(Q, "makeComparable");

// ../convex/dist/esm/index.js
var w = "1.31.3";

// ../convex/dist/esm/server/impl/syscall.js
function H(e, t) {
  if (typeof Convex > "u" || Convex.syscall === void 0)
    throw new Error(
      "The Convex database and auth objects are being used outside of a Convex backend. Did you mean to use `useQuery` or `useMutation` to call a Convex function?"
    );
  let r = Convex.syscall(e, JSON.stringify(t));
  return JSON.parse(r);
}
n(H, "performSyscall");
async function f(e, t) {
  if (typeof Convex > "u" || Convex.asyncSyscall === void 0)
    throw new Error(
      "The Convex database and auth objects are being used outside of a Convex backend. Did you mean to use `useQuery` or `useMutation` to call a Convex function?"
    );
  let r;
  try {
    r = await Convex.asyncSyscall(e, JSON.stringify(t));
  } catch (o) {
    if (o.data !== void 0) {
      let s = new ce(o.message);
      throw s.data = y(o.data), s;
    }
    throw new Error(o.message);
  }
  return JSON.parse(r);
}
n(f, "performAsyncSyscall");
function C(e, t) {
  if (typeof Convex > "u" || Convex.jsSyscall === void 0)
    throw new Error(
      "The Convex database and auth objects are being used outside of a Convex backend. Did you mean to use `useQuery` or `useMutation` to call a Convex function?"
    );
  return Convex.jsSyscall(e, t);
}
n(C, "performJsSyscall");

// ../convex/dist/esm/server/functionName.js
var $ = Symbol.for("functionName");

// ../convex/dist/esm/server/components/paths.js
var $e = Symbol.for("toReferencePath");
function cr(e) {
  return e[$e] ?? null;
}
n(cr, "extractReferencePath");
function lr(e) {
  return e.startsWith("function://");
}
n(lr, "isFunctionHandle");
function A(e) {
  let t;
  if (typeof e == "string")
    lr(e) ? t = { functionHandle: e } : t = { name: e };
  else if (e[$])
    t = { name: e[$] };
  else {
    let r = cr(e);
    if (!r)
      throw new Error(`${e} is not a functionReference`);
    t = { reference: r };
  }
  return t;
}
n(A, "getFunctionAddress");

// ../convex/dist/esm/server/impl/actions_impl.js
function Ne(e, t, r) {
  return {
    ...A(t),
    args: h(O(r)),
    version: w,
    requestId: e
  };
}
n(Ne, "syscallArgs");
function Pe(e) {
  return {
    runQuery: /* @__PURE__ */ n(async (t, r) => {
      let o = await f(
        "1.0/actions/query",
        Ne(e, t, r)
      );
      return y(o);
    }, "runQuery"),
    runMutation: /* @__PURE__ */ n(async (t, r) => {
      let o = await f(
        "1.0/actions/mutation",
        Ne(e, t, r)
      );
      return y(o);
    }, "runMutation"),
    runAction: /* @__PURE__ */ n(async (t, r) => {
      let o = await f(
        "1.0/actions/action",
        Ne(e, t, r)
      );
      return y(o);
    }, "runAction")
  };
}
n(Pe, "setupActionCalls");

// ../convex/dist/esm/server/vector_search.js
var fr = Object.defineProperty, pr = /* @__PURE__ */ n((e, t, r) => t in e ? fr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), yt = /* @__PURE__ */ n((e, t, r) => pr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), le = class {
  static {
    n(this, "FilterExpression");
  }
  /**
   * @internal
   */
  constructor() {
    yt(this, "_isExpression"), yt(this, "_value");
  }
};

// ../convex/dist/esm/server/impl/validate.js
function l(e, t, r, o) {
  if (e === void 0)
    throw new TypeError(
      `Must provide arg ${t} \`${o}\` to \`${r}\``
    );
}
n(l, "validateArg");
function wt(e, t, r, o) {
  if (!Number.isInteger(e) || e < 0)
    throw new TypeError(
      `Arg ${t} \`${o}\` to \`${r}\` must be a non-negative integer`
    );
}
n(wt, "validateArgIsNonNegativeInteger");

// ../convex/dist/esm/server/impl/vector_search_impl.js
var dr = Object.defineProperty, hr = /* @__PURE__ */ n((e, t, r) => t in e ? dr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), Fe = /* @__PURE__ */ n((e, t, r) => hr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField");
function qe(e) {
  return async (t, r, o) => {
    if (l(t, 1, "vectorSearch", "tableName"), l(r, 2, "vectorSearch", "indexName"), l(o, 3, "vectorSearch", "query"), !o.vector || !Array.isArray(o.vector) || o.vector.length === 0)
      throw Error("`vector` must be a non-empty Array in vectorSearch");
    return await new Re(
      e,
      t + "." + r,
      o
    ).collect();
  };
}
n(qe, "setupActionVectorSearch");
var Re = class {
  static {
    n(this, "VectorQueryImpl");
  }
  constructor(t, r, o) {
    Fe(this, "requestId"), Fe(this, "state"), this.requestId = t;
    let s = o.filter ? fe(o.filter(mr)) : null;
    this.state = {
      type: "preparing",
      query: {
        indexName: r,
        limit: o.limit,
        vector: o.vector,
        expressions: s
      }
    };
  }
  async collect() {
    if (this.state.type === "consumed")
      throw new Error("This query is closed and can't emit any more values.");
    let t = this.state.query;
    this.state = { type: "consumed" };
    let { results: r } = await f("1.0/actions/vectorSearch", {
      requestId: this.requestId,
      version: w,
      query: t
    });
    return r;
  }
}, q = class extends le {
  static {
    n(this, "ExpressionImpl");
  }
  constructor(t) {
    super(), Fe(this, "inner"), this.inner = t;
  }
  serialize() {
    return this.inner;
  }
};
function fe(e) {
  return e instanceof q ? e.serialize() : { $literal: v(e) };
}
n(fe, "serializeExpression");
var mr = {
  //  Comparisons  /////////////////////////////////////////////////////////////
  eq(e, t) {
    if (typeof e != "string")
      throw new Error("The first argument to `q.eq` must be a field name.");
    return new q({
      $eq: [
        fe(new q({ $field: e })),
        fe(t)
      ]
    });
  },
  //  Logic  ///////////////////////////////////////////////////////////////////
  or(...e) {
    return new q({ $or: e.map(fe) });
  }
};

// ../convex/dist/esm/server/impl/authentication_impl.js
function G(e) {
  return {
    getUserIdentity: /* @__PURE__ */ n(async () => await f("1.0/getUserIdentity", {
      requestId: e
    }), "getUserIdentity")
  };
}
n(G, "setupAuth");

// ../convex/dist/esm/server/filter_builder.js
var yr = Object.defineProperty, wr = /* @__PURE__ */ n((e, t, r) => t in e ? yr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), xt = /* @__PURE__ */ n((e, t, r) => wr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), pe = class {
  static {
    n(this, "Expression");
  }
  /**
   * @internal
   */
  constructor() {
    xt(this, "_isExpression"), xt(this, "_value");
  }
};

// ../convex/dist/esm/server/impl/filter_builder_impl.js
var xr = Object.defineProperty, gr = /* @__PURE__ */ n((e, t, r) => t in e ? xr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), br = /* @__PURE__ */ n((e, t, r) => gr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), x = class extends pe {
  static {
    n(this, "ExpressionImpl");
  }
  constructor(t) {
    super(), br(this, "inner"), this.inner = t;
  }
  serialize() {
    return this.inner;
  }
};
function d(e) {
  return e instanceof x ? e.serialize() : { $literal: v(e) };
}
n(d, "serializeExpression");
var gt = {
  //  Comparisons  /////////////////////////////////////////////////////////////
  eq(e, t) {
    return new x({
      $eq: [d(e), d(t)]
    });
  },
  neq(e, t) {
    return new x({
      $neq: [d(e), d(t)]
    });
  },
  lt(e, t) {
    return new x({
      $lt: [d(e), d(t)]
    });
  },
  lte(e, t) {
    return new x({
      $lte: [d(e), d(t)]
    });
  },
  gt(e, t) {
    return new x({
      $gt: [d(e), d(t)]
    });
  },
  gte(e, t) {
    return new x({
      $gte: [d(e), d(t)]
    });
  },
  //  Arithmetic  //////////////////////////////////////////////////////////////
  add(e, t) {
    return new x({
      $add: [d(e), d(t)]
    });
  },
  sub(e, t) {
    return new x({
      $sub: [d(e), d(t)]
    });
  },
  mul(e, t) {
    return new x({
      $mul: [d(e), d(t)]
    });
  },
  div(e, t) {
    return new x({
      $div: [d(e), d(t)]
    });
  },
  mod(e, t) {
    return new x({
      $mod: [d(e), d(t)]
    });
  },
  neg(e) {
    return new x({ $neg: d(e) });
  },
  //  Logic  ///////////////////////////////////////////////////////////////////
  and(...e) {
    return new x({ $and: e.map(d) });
  },
  or(...e) {
    return new x({ $or: e.map(d) });
  },
  not(e) {
    return new x({ $not: d(e) });
  },
  //  Other  ///////////////////////////////////////////////////////////////////
  field(e) {
    return new x({ $field: e });
  }
};

// ../convex/dist/esm/server/index_range_builder.js
var vr = Object.defineProperty, Ar = /* @__PURE__ */ n((e, t, r) => t in e ? vr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), Er = /* @__PURE__ */ n((e, t, r) => Ar(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), de = class {
  static {
    n(this, "IndexRange");
  }
  /**
   * @internal
   */
  constructor() {
    Er(this, "_isIndexRange");
  }
};

// ../convex/dist/esm/server/impl/index_range_builder_impl.js
var Ir = Object.defineProperty, Sr = /* @__PURE__ */ n((e, t, r) => t in e ? Ir(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), bt = /* @__PURE__ */ n((e, t, r) => Sr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), he = class e extends de {
  static {
    n(this, "IndexRangeBuilderImpl");
  }
  constructor(t) {
    super(), bt(this, "rangeExpressions"), bt(this, "isConsumed"), this.rangeExpressions = t, this.isConsumed = !1;
  }
  static new() {
    return new e([]);
  }
  consume() {
    if (this.isConsumed)
      throw new Error(
        "IndexRangeBuilder has already been used! Chain your method calls like `q => q.eq(...).eq(...)`. See https://docs.convex.dev/using/indexes"
      );
    this.isConsumed = !0;
  }
  eq(t, r) {
    return this.consume(), new e(
      this.rangeExpressions.concat({
        type: "Eq",
        fieldPath: t,
        value: v(r)
      })
    );
  }
  gt(t, r) {
    return this.consume(), new e(
      this.rangeExpressions.concat({
        type: "Gt",
        fieldPath: t,
        value: v(r)
      })
    );
  }
  gte(t, r) {
    return this.consume(), new e(
      this.rangeExpressions.concat({
        type: "Gte",
        fieldPath: t,
        value: v(r)
      })
    );
  }
  lt(t, r) {
    return this.consume(), new e(
      this.rangeExpressions.concat({
        type: "Lt",
        fieldPath: t,
        value: v(r)
      })
    );
  }
  lte(t, r) {
    return this.consume(), new e(
      this.rangeExpressions.concat({
        type: "Lte",
        fieldPath: t,
        value: v(r)
      })
    );
  }
  export() {
    return this.consume(), this.rangeExpressions;
  }
};

// ../convex/dist/esm/server/search_filter_builder.js
var Or = Object.defineProperty, Tr = /* @__PURE__ */ n((e, t, r) => t in e ? Or(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), _r = /* @__PURE__ */ n((e, t, r) => Tr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), me = class {
  static {
    n(this, "SearchFilter");
  }
  /**
   * @internal
   */
  constructor() {
    _r(this, "_isSearchFilter");
  }
};

// ../convex/dist/esm/server/impl/search_filter_builder_impl.js
var Cr = Object.defineProperty, $r = /* @__PURE__ */ n((e, t, r) => t in e ? Cr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), vt = /* @__PURE__ */ n((e, t, r) => $r(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), ye = class e extends me {
  static {
    n(this, "SearchFilterBuilderImpl");
  }
  constructor(t) {
    super(), vt(this, "filters"), vt(this, "isConsumed"), this.filters = t, this.isConsumed = !1;
  }
  static new() {
    return new e([]);
  }
  consume() {
    if (this.isConsumed)
      throw new Error(
        "SearchFilterBuilder has already been used! Chain your method calls like `q => q.search(...).eq(...)`."
      );
    this.isConsumed = !0;
  }
  search(t, r) {
    return l(t, 1, "search", "fieldName"), l(r, 2, "search", "query"), this.consume(), new e(
      this.filters.concat({
        type: "Search",
        fieldPath: t,
        value: r
      })
    );
  }
  eq(t, r) {
    return l(t, 1, "eq", "fieldName"), arguments.length !== 2 && l(r, 2, "search", "value"), this.consume(), new e(
      this.filters.concat({
        type: "Eq",
        fieldPath: t,
        value: v(r)
      })
    );
  }
  export() {
    return this.consume(), this.filters;
  }
};

// ../convex/dist/esm/server/impl/query_impl.js
var Nr = Object.defineProperty, Pr = /* @__PURE__ */ n((e, t, r) => t in e ? Nr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), Be = /* @__PURE__ */ n((e, t, r) => Pr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), At = 256, B = class {
  static {
    n(this, "QueryInitializerImpl");
  }
  constructor(t) {
    Be(this, "tableName"), this.tableName = t;
  }
  withIndex(t, r) {
    l(t, 1, "withIndex", "indexName");
    let o = he.new();
    return r !== void 0 && (o = r(o)), new N({
      source: {
        type: "IndexRange",
        indexName: this.tableName + "." + t,
        range: o.export(),
        order: null
      },
      operators: []
    });
  }
  withSearchIndex(t, r) {
    l(t, 1, "withSearchIndex", "indexName"), l(r, 2, "withSearchIndex", "searchFilter");
    let o = ye.new();
    return new N({
      source: {
        type: "Search",
        indexName: this.tableName + "." + t,
        filters: r(o).export()
      },
      operators: []
    });
  }
  fullTableScan() {
    return new N({
      source: {
        type: "FullTableScan",
        tableName: this.tableName,
        order: null
      },
      operators: []
    });
  }
  order(t) {
    return this.fullTableScan().order(t);
  }
  // This is internal API and should not be exposed to developers yet.
  async count() {
    let t = await f("1.0/count", {
      table: this.tableName
    });
    return y(t);
  }
  filter(t) {
    return this.fullTableScan().filter(t);
  }
  limit(t) {
    return this.fullTableScan().limit(t);
  }
  collect() {
    return this.fullTableScan().collect();
  }
  take(t) {
    return this.fullTableScan().take(t);
  }
  paginate(t) {
    return this.fullTableScan().paginate(t);
  }
  first() {
    return this.fullTableScan().first();
  }
  unique() {
    return this.fullTableScan().unique();
  }
  [Symbol.asyncIterator]() {
    return this.fullTableScan()[Symbol.asyncIterator]();
  }
};
function Et(e) {
  throw new Error(
    e === "consumed" ? "This query is closed and can't emit any more values." : "This query has been chained with another operator and can't be reused."
  );
}
n(Et, "throwClosedError");
var N = class e {
  static {
    n(this, "QueryImpl");
  }
  constructor(t) {
    Be(this, "state"), Be(this, "tableNameForErrorMessages"), this.state = { type: "preparing", query: t }, t.source.type === "FullTableScan" ? this.tableNameForErrorMessages = t.source.tableName : this.tableNameForErrorMessages = t.source.indexName.split(".")[0];
  }
  takeQuery() {
    if (this.state.type !== "preparing")
      throw new Error(
        "A query can only be chained once and can't be chained after iteration begins."
      );
    let t = this.state.query;
    return this.state = { type: "closed" }, t;
  }
  startQuery() {
    if (this.state.type === "executing")
      throw new Error("Iteration can only begin on a query once.");
    (this.state.type === "closed" || this.state.type === "consumed") && Et(this.state.type);
    let t = this.state.query, { queryId: r } = H("1.0/queryStream", { query: t, version: w });
    return this.state = { type: "executing", queryId: r }, r;
  }
  closeQuery() {
    if (this.state.type === "executing") {
      let t = this.state.queryId;
      H("1.0/queryCleanup", { queryId: t });
    }
    this.state = { type: "consumed" };
  }
  order(t) {
    l(t, 1, "order", "order");
    let r = this.takeQuery();
    if (r.source.type === "Search")
      throw new Error(
        "Search queries must always be in relevance order. Can not set order manually."
      );
    if (r.source.order !== null)
      throw new Error("Queries may only specify order at most once");
    return r.source.order = t, new e(r);
  }
  filter(t) {
    l(t, 1, "filter", "predicate");
    let r = this.takeQuery();
    if (r.operators.length >= At)
      throw new Error(
        `Can't construct query with more than ${At} operators`
      );
    return r.operators.push({
      filter: d(t(gt))
    }), new e(r);
  }
  limit(t) {
    l(t, 1, "limit", "n");
    let r = this.takeQuery();
    return r.operators.push({ limit: t }), new e(r);
  }
  [Symbol.asyncIterator]() {
    return this.startQuery(), this;
  }
  async next() {
    (this.state.type === "closed" || this.state.type === "consumed") && Et(this.state.type);
    let t = this.state.type === "preparing" ? this.startQuery() : this.state.queryId, { value: r, done: o } = await f("1.0/queryStreamNext", {
      queryId: t
    });
    return o && this.closeQuery(), { value: y(r), done: o };
  }
  return() {
    return this.closeQuery(), Promise.resolve({ done: !0, value: void 0 });
  }
  async paginate(t) {
    if (l(t, 1, "paginate", "options"), typeof t?.numItems != "number" || t.numItems < 0)
      throw new Error(
        `\`options.numItems\` must be a positive number. Received \`${t?.numItems}\`.`
      );
    let r = this.takeQuery(), o = t.numItems, s = t.cursor, i = t?.endCursor ?? null, a = t.maximumRowsRead ?? null, { page: u, isDone: p, continueCursor: I, splitCursor: F, pageStatus: ve } = await f("1.0/queryPage", {
      query: r,
      cursor: s,
      endCursor: i,
      pageSize: o,
      maximumRowsRead: a,
      maximumBytesRead: t.maximumBytesRead,
      version: w
    });
    return {
      page: u.map((Mt) => y(Mt)),
      isDone: p,
      continueCursor: I,
      splitCursor: F,
      pageStatus: ve
    };
  }
  async collect() {
    let t = [];
    for await (let r of this)
      t.push(r);
    return t;
  }
  async take(t) {
    return l(t, 1, "take", "n"), wt(t, 1, "take", "n"), this.limit(t).collect();
  }
  async first() {
    let t = await this.take(1);
    return t.length === 0 ? null : t[0];
  }
  async unique() {
    let t = await this.take(2);
    if (t.length === 0)
      return null;
    if (t.length === 2)
      throw new Error(`unique() query returned more than one result from table ${this.tableNameForErrorMessages}:
 [${t[0]._id}, ${t[1]._id}, ...]`);
    return t[0];
  }
};

// ../convex/dist/esm/server/impl/database_impl.js
async function je(e, t, r) {
  if (l(t, 1, "get", "id"), typeof t != "string")
    throw new Error(
      `Invalid argument \`id\` for \`db.get\`, expected string but got '${typeof t}': ${t}`
    );
  let o = {
    id: h(t),
    isSystem: r,
    version: w,
    table: e
  }, s = await f("1.0/get", o);
  return y(s);
}
n(je, "get");
function Le() {
  let e = /* @__PURE__ */ n((s = !1) => ({
    get: /* @__PURE__ */ n(async (i, a) => a !== void 0 ? await je(i, a, s) : await je(void 0, i, s), "get"),
    query: /* @__PURE__ */ n((i) => new W(i, s).query(), "query"),
    normalizeId: /* @__PURE__ */ n((i, a) => {
      l(i, 1, "normalizeId", "tableName"), l(a, 2, "normalizeId", "id");
      let u = i.startsWith("_");
      if (u !== s)
        throw new Error(
          `${u ? "System" : "User"} tables can only be accessed from db.${s ? "" : "system."}normalizeId().`
        );
      let p = H("1.0/db/normalizeId", {
        table: i,
        idString: a
      });
      return y(p).id;
    }, "normalizeId"),
    // We set the system reader on the next line
    system: null,
    table: /* @__PURE__ */ n((i) => new W(i, s), "table")
  }), "reader"), { system: t, ...r } = e(!0), o = e();
  return o.system = r, o;
}
n(Le, "setupReader");
async function It(e, t) {
  if (e.startsWith("_"))
    throw new Error("System tables (prefixed with `_`) are read-only.");
  l(e, 1, "insert", "table"), l(t, 2, "insert", "value");
  let r = await f("1.0/insert", {
    table: e,
    value: h(t)
  });
  return y(r)._id;
}
n(It, "insert");
async function Me(e, t, r) {
  l(t, 1, "patch", "id"), l(r, 2, "patch", "value"), await f("1.0/shallowMerge", {
    id: h(t),
    value: at(r),
    table: e
  });
}
n(Me, "patch");
async function Ue(e, t, r) {
  l(t, 1, "replace", "id"), l(r, 2, "replace", "value"), await f("1.0/replace", {
    id: h(t),
    value: h(r),
    table: e
  });
}
n(Ue, "replace");
async function Je(e, t) {
  l(t, 1, "delete", "id"), await f("1.0/remove", {
    id: h(t),
    table: e
  });
}
n(Je, "delete_");
function St() {
  let e = Le();
  return {
    get: e.get,
    query: e.query,
    normalizeId: e.normalizeId,
    system: e.system,
    insert: /* @__PURE__ */ n(async (t, r) => await It(t, r), "insert"),
    patch: /* @__PURE__ */ n(async (t, r, o) => o !== void 0 ? await Me(t, r, o) : await Me(void 0, t, r), "patch"),
    replace: /* @__PURE__ */ n(async (t, r, o) => o !== void 0 ? await Ue(t, r, o) : await Ue(void 0, t, r), "replace"),
    delete: /* @__PURE__ */ n(async (t, r) => r !== void 0 ? await Je(t, r) : await Je(void 0, t), "delete"),
    table: /* @__PURE__ */ n((t) => new Ve(t, !1), "table")
  };
}
n(St, "setupWriter");
var W = class {
  static {
    n(this, "TableReader");
  }
  constructor(t, r) {
    this.tableName = t, this.isSystem = r;
  }
  async get(t) {
    return je(this.tableName, t, this.isSystem);
  }
  query() {
    let t = this.tableName.startsWith("_");
    if (t !== this.isSystem)
      throw new Error(
        `${t ? "System" : "User"} tables can only be accessed from db.${this.isSystem ? "" : "system."}query().`
      );
    return new B(this.tableName);
  }
}, Ve = class extends W {
  static {
    n(this, "TableWriter");
  }
  async insert(t) {
    return It(this.tableName, t);
  }
  async patch(t, r) {
    return Me(this.tableName, t, r);
  }
  async replace(t, r) {
    return Ue(this.tableName, t, r);
  }
  async delete(t) {
    return Je(this.tableName, t);
  }
};

// ../convex/dist/esm/server/impl/scheduler_impl.js
function Ot() {
  return {
    runAfter: /* @__PURE__ */ n(async (e, t, r) => {
      let o = Tt(e, t, r);
      return await f("1.0/schedule", o);
    }, "runAfter"),
    runAt: /* @__PURE__ */ n(async (e, t, r) => {
      let o = _t(
        e,
        t,
        r
      );
      return await f("1.0/schedule", o);
    }, "runAt"),
    cancel: /* @__PURE__ */ n(async (e) => {
      l(e, 1, "cancel", "id");
      let t = { id: h(e) };
      await f("1.0/cancel_job", t);
    }, "cancel")
  };
}
n(Ot, "setupMutationScheduler");
function ke(e) {
  return {
    runAfter: /* @__PURE__ */ n(async (t, r, o) => {
      let s = {
        requestId: e,
        ...Tt(t, r, o)
      };
      return await f("1.0/actions/schedule", s);
    }, "runAfter"),
    runAt: /* @__PURE__ */ n(async (t, r, o) => {
      let s = {
        requestId: e,
        ..._t(t, r, o)
      };
      return await f("1.0/actions/schedule", s);
    }, "runAt"),
    cancel: /* @__PURE__ */ n(async (t) => {
      l(t, 1, "cancel", "id");
      let r = { id: h(t) };
      return await f("1.0/actions/cancel_job", r);
    }, "cancel")
  };
}
n(ke, "setupActionScheduler");
function Tt(e, t, r) {
  if (typeof e != "number")
    throw new Error("`delayMs` must be a number");
  if (!isFinite(e))
    throw new Error("`delayMs` must be a finite number");
  if (e < 0)
    throw new Error("`delayMs` must be non-negative");
  let o = O(r), s = A(t), i = (Date.now() + e) / 1e3;
  return {
    ...s,
    ts: i,
    args: h(o),
    version: w
  };
}
n(Tt, "runAfterSyscallArgs");
function _t(e, t, r) {
  let o;
  if (e instanceof Date)
    o = e.valueOf() / 1e3;
  else if (typeof e == "number")
    o = e / 1e3;
  else
    throw new Error("The invoke time must a Date or a timestamp");
  let s = A(t), i = O(r);
  return {
    ...s,
    ts: o,
    args: h(i),
    version: w
  };
}
n(_t, "runAtSyscallArgs");

// ../convex/dist/esm/server/impl/storage_impl.js
function De(e) {
  return {
    getUrl: /* @__PURE__ */ n(async (t) => (l(t, 1, "getUrl", "storageId"), await f("1.0/storageGetUrl", {
      requestId: e,
      version: w,
      storageId: t
    })), "getUrl"),
    getMetadata: /* @__PURE__ */ n(async (t) => await f("1.0/storageGetMetadata", {
      requestId: e,
      version: w,
      storageId: t
    }), "getMetadata")
  };
}
n(De, "setupStorageReader");
function Qe(e) {
  let t = De(e);
  return {
    generateUploadUrl: /* @__PURE__ */ n(async () => await f("1.0/storageGenerateUploadUrl", {
      requestId: e,
      version: w
    }), "generateUploadUrl"),
    delete: /* @__PURE__ */ n(async (r) => {
      await f("1.0/storageDelete", {
        requestId: e,
        version: w,
        storageId: r
      });
    }, "delete"),
    getUrl: t.getUrl,
    getMetadata: t.getMetadata
  };
}
n(Qe, "setupStorageWriter");
function He(e) {
  return {
    ...Qe(e),
    store: /* @__PURE__ */ n(async (r, o) => await C("storage/storeBlob", {
      requestId: e,
      version: w,
      blob: r,
      options: o
    }), "store"),
    get: /* @__PURE__ */ n(async (r) => await C("storage/getBlob", {
      requestId: e,
      version: w,
      storageId: r
    }), "get")
  };
}
n(He, "setupStorageActionWriter");

// ../convex/dist/esm/server/impl/registration_impl.js
async function Ct(e, t) {
  let o = y(JSON.parse(t)), s = {
    db: St(),
    auth: G(""),
    storage: Qe(""),
    scheduler: Ot(),
    runQuery: /* @__PURE__ */ n((a, u) => Ge("query", a, u), "runQuery"),
    runMutation: /* @__PURE__ */ n((a, u) => Ge("mutation", a, u), "runMutation")
  }, i = await we(e, s, o);
  return $t(i), JSON.stringify(h(i === void 0 ? null : i));
}
n(Ct, "invokeMutation");
function $t(e) {
  if (e instanceof B || e instanceof N)
    throw new Error(
      "Return value is a Query. Results must be retrieved with `.collect()`, `.take(n), `.unique()`, or `.first()`."
    );
}
n($t, "validateReturnValue");
async function we(e, t, r) {
  let o;
  try {
    o = await Promise.resolve(e(t, ...r));
  } catch (s) {
    throw Fr(s);
  }
  return o;
}
n(we, "invokeFunction");
function j(e, t) {
  return (r, o) => (globalThis.console.warn(
    `Convex functions should not directly call other Convex functions. Consider calling a helper function instead. e.g. \`export const foo = ${e}(...); await foo(ctx);\` is not supported. See https://docs.convex.dev/production/best-practices/#use-helper-functions-to-write-shared-code`
  ), t(r, o));
}
n(j, "dontCallDirectly");
function Fr(e) {
  if (typeof e == "object" && e !== null && Symbol.for("ConvexError") in e) {
    let t = e;
    return t.data = JSON.stringify(
      h(t.data === void 0 ? null : t.data)
    ), t.ConvexErrorSymbol = Symbol.for("ConvexError"), t;
  } else
    return e;
}
n(Fr, "serializeConvexErrorData");
function M() {
  if (typeof window > "u" || window.__convexAllowFunctionsInBrowser)
    return;
  (Object.getOwnPropertyDescriptor(globalThis, "window")?.get?.toString().includes("[native code]") ?? !1) && console.error(
    "Convex functions should not be imported in the browser. This will throw an error in future versions of `convex`. If this is a false negative, please report it to Convex support."
  );
}
n(M, "assertNotBrowser");
function Nt(e, t) {
  if (t === void 0)
    throw new Error(
      `A validator is undefined for field "${e}". This is often caused by circular imports. See https://docs.convex.dev/error#undefined-validator for details.`
    );
  return t;
}
n(Nt, "strictReplacer");
function z(e) {
  return () => {
    let t = c.any();
    return typeof e == "object" && e.args !== void 0 && (t = ue(e.args)), JSON.stringify(t.json, Nt);
  };
}
n(z, "exportArgs");
function X(e) {
  return () => {
    let t;
    return typeof e == "object" && e.returns !== void 0 && (t = ue(e.returns)), JSON.stringify(t ? t.json : null, Nt);
  };
}
n(X, "exportReturns");
var Rr = /* @__PURE__ */ n(((e) => {
  let t = typeof e == "function" ? e : e.handler, r = j("mutation", t);
  return M(), r.isMutation = !0, r.isPublic = !0, r.invokeMutation = (o) => Ct(t, o), r.exportArgs = z(e), r.exportReturns = X(e), r._handler = t, r;
}), "mutationGeneric"), qr = /* @__PURE__ */ n(((e) => {
  let t = typeof e == "function" ? e : e.handler, r = j(
    "internalMutation",
    t
  );
  return M(), r.isMutation = !0, r.isInternal = !0, r.invokeMutation = (o) => Ct(t, o), r.exportArgs = z(e), r.exportReturns = X(e), r._handler = t, r;
}), "internalMutationGeneric");
async function Pt(e, t) {
  let o = y(JSON.parse(t)), s = {
    db: Le(),
    auth: G(""),
    storage: De(""),
    runQuery: /* @__PURE__ */ n((a, u) => Ge("query", a, u), "runQuery")
  }, i = await we(e, s, o);
  return $t(i), JSON.stringify(h(i === void 0 ? null : i));
}
n(Pt, "invokeQuery");
var Br = /* @__PURE__ */ n(((e) => {
  let t = typeof e == "function" ? e : e.handler, r = j("query", t);
  return M(), r.isQuery = !0, r.isPublic = !0, r.invokeQuery = (o) => Pt(t, o), r.exportArgs = z(e), r.exportReturns = X(e), r._handler = t, r;
}), "queryGeneric"), jr = /* @__PURE__ */ n(((e) => {
  let t = typeof e == "function" ? e : e.handler, r = j("internalQuery", t);
  return M(), r.isQuery = !0, r.isInternal = !0, r.invokeQuery = (o) => Pt(t, o), r.exportArgs = z(e), r.exportReturns = X(e), r._handler = t, r;
}), "internalQueryGeneric");
async function Mr(e, t, r) {
  let o = y(JSON.parse(r)), i = {
    ...Pe(t),
    auth: G(t),
    scheduler: ke(t),
    storage: He(t),
    vectorSearch: qe(t)
  }, a = await we(e, i, o);
  return JSON.stringify(h(a === void 0 ? null : a));
}
n(Mr, "invokeAction");
var Ur = /* @__PURE__ */ n(((e) => {
  let t = typeof e == "function" ? e : e.handler, r = j("action", t);
  return M(), r.isAction = !0, r.isPublic = !0, r.invokeAction = (o, s) => Mr(t, o, s), r.exportArgs = z(e), r.exportReturns = X(e), r._handler = t, r;
}), "actionGeneric");
async function Jr(e, t) {
  let s = {
    ...Pe(""),
    auth: G(""),
    storage: He(""),
    scheduler: ke(""),
    vectorSearch: qe("")
  };
  return await we(e, s, [t]);
}
n(Jr, "invokeHttpAction");
var Vr = /* @__PURE__ */ n((e) => {
  let t = j("httpAction", e);
  return M(), t.isHttp = !0, t.invokeHttpAction = (r) => Jr(e, r), t._handler = e, t;
}, "httpActionGeneric");
async function Ge(e, t, r) {
  let o = O(r), s = {
    udfType: e,
    args: h(o),
    ...A(t)
  }, i = await f("1.0/runUdf", s);
  return y(i);
}
n(Ge, "runUdf");

// ../convex/dist/esm/server/pagination.js
var ws = c.object({
  numItems: c.number(),
  cursor: c.union(c.string(), c.null()),
  endCursor: c.optional(c.union(c.string(), c.null())),
  id: c.optional(c.number()),
  maximumRowsRead: c.optional(c.number()),
  maximumBytesRead: c.optional(c.number())
});

// ../convex/dist/esm/server/api.js
function We(e) {
  let t = A(e);
  if (t.name === void 0)
    throw t.functionHandle !== void 0 ? new Error(
      `Expected function reference like "api.file.func" or "internal.file.func", but received function handle ${t.functionHandle}`
    ) : t.reference !== void 0 ? new Error(
      `Expected function reference in the current component like "api.file.func" or "internal.file.func", but received reference ${t.reference}`
    ) : new Error(
      `Expected function reference like "api.file.func" or "internal.file.func", but received ${JSON.stringify(t)}`
    );
  if (typeof e == "string") return e;
  let r = e[$];
  if (!r)
    throw new Error(`${e} is not a functionReference`);
  return r;
}
n(We, "getFunctionName");
function Lr(e) {
  return { [$]: e };
}
n(Lr, "makeFunctionReference");
function Ft(e = []) {
  let t = {
    get(r, o) {
      if (typeof o == "string") {
        let s = [...e, o];
        return Ft(s);
      } else if (o === $) {
        if (e.length < 2) {
          let a = ["api", ...e].join(".");
          throw new Error(
            `API path is expected to be of the form \`api.moduleName.functionName\`. Found: \`${a}\``
          );
        }
        let s = e.slice(0, -1).join("/"), i = e[e.length - 1];
        return i === "default" ? s : s + ":" + i;
      } else return o === Symbol.toStringTag ? "FunctionReference" : void 0;
    }
  };
  return new Proxy({}, t);
}
n(Ft, "createApi");
var kr = Ft();

// ../convex/dist/esm/server/cron.js
var Dr = Object.defineProperty, Qr = /* @__PURE__ */ n((e, t, r) => t in e ? Dr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), Rt = /* @__PURE__ */ n((e, t, r) => Qr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), Hr = [
  "sunday",
  "monday",
  "tuesday",
  "wednesday",
  "thursday",
  "friday",
  "saturday"
], Gr = /* @__PURE__ */ n(() => new Ye(), "cronJobs");
function ze(e) {
  if (!Number.isInteger(e) || e <= 0)
    throw new Error("Interval must be an integer greater than 0");
}
n(ze, "validateIntervalNumber");
function Wr(e) {
  if (!Number.isInteger(e) || e < 1 || e > 31)
    throw new Error("Day of month must be an integer from 1 to 31");
  return e;
}
n(Wr, "validatedDayOfMonth");
function zr(e) {
  if (!Hr.includes(e))
    throw new Error('Day of week must be a string like "monday".');
  return e;
}
n(zr, "validatedDayOfWeek");
function Xe(e) {
  if (!Number.isInteger(e) || e < 0 || e > 23)
    throw new Error("Hour of day must be an integer from 0 to 23");
  return e;
}
n(Xe, "validatedHourOfDay");
function xe(e) {
  if (!Number.isInteger(e) || e < 0 || e > 59)
    throw new Error("Minute of hour must be an integer from 0 to 59");
  return e;
}
n(xe, "validatedMinuteOfHour");
function Xr(e) {
  if (!e.match(/^[ -~]*$/))
    throw new Error(
      `Invalid cron identifier ${e}: use ASCII letters that are not control characters`
    );
  return e;
}
n(Xr, "validatedCronIdentifier");
var Ye = class {
  static {
    n(this, "Crons");
  }
  constructor() {
    Rt(this, "crons"), Rt(this, "isCrons"), this.isCrons = !0, this.crons = {};
  }
  /** @internal */
  schedule(t, r, o, s) {
    let i = O(s);
    if (Xr(t), t in this.crons)
      throw new Error(`Cron identifier registered twice: ${t}`);
    this.crons[t] = {
      name: We(o),
      args: [h(i)],
      schedule: r
    };
  }
  /**
   * Schedule a mutation or action to run at some interval.
   *
   * ```js
   * crons.interval("Clear presence data", {seconds: 30}, api.presence.clear);
   * ```
   *
   * @param identifier - A unique name for this scheduled job.
   * @param schedule - The time between runs for this scheduled job.
   * @param functionReference - A {@link FunctionReference} for the function
   * to schedule.
   * @param args - The arguments to the function.
   */
  interval(t, r, o, ...s) {
    let i = r, a = +("seconds" in i && i.seconds !== void 0), u = +("minutes" in i && i.minutes !== void 0), p = +("hours" in i && i.hours !== void 0);
    if (a + u + p !== 1)
      throw new Error("Must specify one of seconds, minutes, or hours");
    a ? ze(r.seconds) : u ? ze(r.minutes) : p && ze(r.hours), this.schedule(
      t,
      { ...r, type: "interval" },
      o,
      ...s
    );
  }
  /**
   * Schedule a mutation or action to run on an hourly basis.
   *
   * ```js
   * crons.hourly(
   *   "Reset high scores",
   *   {
   *     minuteUTC: 30,
   *   },
   *   api.scores.reset
   * )
   * ```
   *
   * @param cronIdentifier - A unique name for this scheduled job.
   * @param schedule - What time (UTC) each day to run this function.
   * @param functionReference - A {@link FunctionReference} for the function
   * to schedule.
   * @param args - The arguments to the function.
   */
  hourly(t, r, o, ...s) {
    let i = xe(r.minuteUTC);
    this.schedule(
      t,
      { minuteUTC: i, type: "hourly" },
      o,
      ...s
    );
  }
  /**
   * Schedule a mutation or action to run on a daily basis.
   *
   * ```js
   * crons.daily(
   *   "Reset high scores",
   *   {
   *     hourUTC: 17, // (9:30am Pacific/10:30am Daylight Savings Pacific)
   *     minuteUTC: 30,
   *   },
   *   api.scores.reset
   * )
   * ```
   *
   * @param cronIdentifier - A unique name for this scheduled job.
   * @param schedule - What time (UTC) each day to run this function.
   * @param functionReference - A {@link FunctionReference} for the function
   * to schedule.
   * @param args - The arguments to the function.
   */
  daily(t, r, o, ...s) {
    let i = Xe(r.hourUTC), a = xe(r.minuteUTC);
    this.schedule(
      t,
      { hourUTC: i, minuteUTC: a, type: "daily" },
      o,
      ...s
    );
  }
  /**
   * Schedule a mutation or action to run on a weekly basis.
   *
   * ```js
   * crons.weekly(
   *   "Weekly re-engagement email",
   *   {
   *     dayOfWeek: "Tuesday",
   *     hourUTC: 17, // (9:30am Pacific/10:30am Daylight Savings Pacific)
   *     minuteUTC: 30,
   *   },
   *   api.emails.send
   * )
   * ```
   *
   * @param cronIdentifier - A unique name for this scheduled job.
   * @param schedule - What day and time (UTC) each week to run this function.
   * @param functionReference - A {@link FunctionReference} for the function
   * to schedule.
   */
  weekly(t, r, o, ...s) {
    let i = zr(r.dayOfWeek), a = Xe(r.hourUTC), u = xe(r.minuteUTC);
    this.schedule(
      t,
      { dayOfWeek: i, hourUTC: a, minuteUTC: u, type: "weekly" },
      o,
      ...s
    );
  }
  /**
   * Schedule a mutation or action to run on a monthly basis.
   *
   * Note that some months have fewer days than others, so e.g. a function
   * scheduled to run on the 30th will not run in February.
   *
   * ```js
   * crons.monthly(
   *   "Bill customers at ",
   *   {
   *     hourUTC: 17, // (9:30am Pacific/10:30am Daylight Savings Pacific)
   *     minuteUTC: 30,
   *     day: 1,
   *   },
   *   api.billing.billCustomers
   * )
   * ```
   *
   * @param cronIdentifier - A unique name for this scheduled job.
   * @param schedule - What day and time (UTC) each month to run this function.
   * @param functionReference - A {@link FunctionReference} for the function
   * to schedule.
   * @param args - The arguments to the function.
   */
  monthly(t, r, o, ...s) {
    let i = Wr(r.day), a = Xe(r.hourUTC), u = xe(r.minuteUTC);
    this.schedule(
      t,
      { day: i, hourUTC: a, minuteUTC: u, type: "monthly" },
      o,
      ...s
    );
  }
  /**
   * Schedule a mutation or action to run on a recurring basis.
   *
   * Like the unix command `cron`, Sunday is 0, Monday is 1, etc.
   *
   * ```
   *  ┌─ minute (0 - 59)
   *  │ ┌─ hour (0 - 23)
   *  │ │ ┌─ day of the month (1 - 31)
   *  │ │ │ ┌─ month (1 - 12)
   *  │ │ │ │ ┌─ day of the week (0 - 6) (Sunday to Saturday)
   * "* * * * *"
   * ```
   *
   * @param cronIdentifier - A unique name for this scheduled job.
   * @param cron - Cron string like `"15 7 * * *"` (Every day at 7:15 UTC)
   * @param functionReference - A {@link FunctionReference} for the function
   * to schedule.
   * @param args - The arguments to the function.
   */
  cron(t, r, o, ...s) {
    let i = r;
    this.schedule(
      t,
      { cron: i, type: "cron" },
      o,
      ...s
    );
  }
  /** @internal */
  export() {
    return JSON.stringify(this.crons);
  }
};

// ../convex/dist/esm/server/router.js
var Yr = Object.defineProperty, Kr = /* @__PURE__ */ n((e, t, r) => t in e ? Yr(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), P = /* @__PURE__ */ n((e, t, r) => Kr(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), qt = [
  "GET",
  "POST",
  "PUT",
  "DELETE",
  "OPTIONS",
  "PATCH"
];
function Zr(e) {
  return e === "HEAD" ? "GET" : e;
}
n(Zr, "normalizeMethod");
var en = /* @__PURE__ */ n(() => new ge(), "httpRouter"), ge = class {
  static {
    n(this, "HttpRouter");
  }
  constructor() {
    P(this, "exactRoutes", /* @__PURE__ */ new Map()), P(this, "prefixRoutes", /* @__PURE__ */ new Map()), P(this, "isRouter", !0), P(this, "route", (t) => {
      if (!t.handler) throw new Error("route requires handler");
      if (!t.method) throw new Error("route requires method");
      let { method: r, handler: o } = t;
      if (!qt.includes(r))
        throw new Error(
          `'${r}' is not an allowed HTTP method (like GET, POST, PUT etc.)`
        );
      if ("path" in t) {
        if ("pathPrefix" in t)
          throw new Error(
            "Invalid httpRouter route: cannot contain both 'path' and 'pathPrefix'"
          );
        if (!t.path.startsWith("/"))
          throw new Error(`path '${t.path}' does not start with a /`);
        if (t.path.startsWith("/.files/") || t.path === "/.files")
          throw new Error(`path '${t.path}' is reserved`);
        let s = this.exactRoutes.has(t.path) ? this.exactRoutes.get(t.path) : /* @__PURE__ */ new Map();
        if (s.has(r))
          throw new Error(
            `Path '${t.path}' for method ${r} already in use`
          );
        s.set(r, o), this.exactRoutes.set(t.path, s);
      } else if ("pathPrefix" in t) {
        if (!t.pathPrefix.startsWith("/"))
          throw new Error(
            `pathPrefix '${t.pathPrefix}' does not start with a /`
          );
        if (!t.pathPrefix.endsWith("/"))
          throw new Error(`pathPrefix ${t.pathPrefix} must end with a /`);
        if (t.pathPrefix.startsWith("/.files/"))
          throw new Error(`pathPrefix '${t.pathPrefix}' is reserved`);
        let s = this.prefixRoutes.get(r) || /* @__PURE__ */ new Map();
        if (s.has(t.pathPrefix))
          throw new Error(
            `${t.method} pathPrefix ${t.pathPrefix} is already defined`
          );
        s.set(t.pathPrefix, o), this.prefixRoutes.set(r, s);
      } else
        throw new Error(
          "Invalid httpRouter route entry: must contain either field 'path' or 'pathPrefix'"
        );
    }), P(this, "getRoutes", () => {
      let r = [...this.exactRoutes.keys()].sort().flatMap(
        (i) => [...this.exactRoutes.get(i).keys()].sort().map(
          (a) => [i, a, this.exactRoutes.get(i).get(a)]
        )
      ), s = [...this.prefixRoutes.keys()].sort().flatMap(
        (i) => [...this.prefixRoutes.get(i).keys()].sort().map(
          (a) => [
            `${a}*`,
            i,
            this.prefixRoutes.get(i).get(a)
          ]
        )
      );
      return [...r, ...s];
    }), P(this, "lookup", (t, r) => {
      r = Zr(r);
      let o = this.exactRoutes.get(t)?.get(r);
      if (o) return [o, r, t];
      let i = [...(this.prefixRoutes.get(r) || /* @__PURE__ */ new Map()).entries()].sort(
        ([a, u], [p, I]) => p.length - a.length
      );
      for (let [a, u] of i)
        if (t.startsWith(a))
          return [u, r, `${a}*`];
      return null;
    }), P(this, "runRequest", async (t, r) => {
      let o = C("requestFromConvexJson", {
        convexJson: JSON.parse(t)
      }), s = r;
      (!s || typeof s != "string") && (s = new URL(o.url).pathname);
      let i = o.method, a = this.lookup(s, i);
      if (!a) {
        let ve = new Response(`No HttpAction routed for ${s}`, {
          status: 404
        });
        return JSON.stringify(
          C("convexJsonFromResponse", { response: ve })
        );
      }
      let [u, p, I] = a, F = await u.invokeHttpAction(o);
      return JSON.stringify(
        C("convexJsonFromResponse", { response: F })
      );
    });
  }
};

// ../convex/dist/esm/server/schema.js
var tn = Object.defineProperty, rn = /* @__PURE__ */ n((e, t, r) => t in e ? tn(e, t, { enumerable: !0, configurable: !0, writable: !0, value: r }) : e[t] = r, "__defNormalProp"), E = /* @__PURE__ */ n((e, t, r) => rn(e, typeof t != "symbol" ? t + "" : t, r), "__publicField"), be = class {
  static {
    n(this, "TableDefinition");
  }
  /**
   * @internal
   */
  constructor(t) {
    E(this, "indexes"), E(this, "stagedDbIndexes"), E(this, "searchIndexes"), E(this, "stagedSearchIndexes"), E(this, "vectorIndexes"), E(this, "stagedVectorIndexes"), E(this, "_monotonicCreationTime", !1), E(this, "validator"), this.indexes = [], this.stagedDbIndexes = [], this.searchIndexes = [], this.stagedSearchIndexes = [], this.vectorIndexes = [], this.stagedVectorIndexes = [], this.validator = t;
  }
  /**
   * This API is experimental: it may change or disappear.
   *
   * Returns indexes defined on this table.
   * Intended for the advanced use cases of dynamically deciding which index to use for a query.
   * If you think you need this, please chime in on ths issue in the Convex JS GitHub repo.
   * https://github.com/get-convex/convex-js/issues/49
   */
  " indexes"() {
    return this.indexes;
  }
  index(t, r) {
    return Array.isArray(r) ? this.indexes.push({
      indexDescriptor: t,
      fields: r
    }) : r.staged ? this.stagedDbIndexes.push({
      indexDescriptor: t,
      fields: r.fields
    }) : this.indexes.push({
      indexDescriptor: t,
      fields: r.fields
    }), this;
  }
  searchIndex(t, r) {
    return r.staged ? this.stagedSearchIndexes.push({
      indexDescriptor: t,
      searchField: r.searchField,
      filterFields: r.filterFields || []
    }) : this.searchIndexes.push({
      indexDescriptor: t,
      searchField: r.searchField,
      filterFields: r.filterFields || []
    }), this;
  }
  vectorIndex(t, r) {
    return r.staged ? this.stagedVectorIndexes.push({
      indexDescriptor: t,
      vectorField: r.vectorField,
      dimensions: r.dimensions,
      filterFields: r.filterFields || []
    }) : this.vectorIndexes.push({
      indexDescriptor: t,
      vectorField: r.vectorField,
      dimensions: r.dimensions,
      filterFields: r.filterFields || []
    }), this;
  }
  /**
   * Enable monotonic creation times for this table.
   *
   * When enabled, the `_creationTime` field for documents in this table will be
   * guaranteed to be monotonically increasing in commit order, rather than
   * transaction execution order. This ensures that documents committed later
   * will always have a higher `_creationTime` than documents committed earlier,
   * even if their transactions executed concurrently.
   *
   * @returns A {@link TableDefinition} with monotonic creation times enabled.
   */
  monotonicCreationTime() {
    return this._monotonicCreationTime = !0, this;
  }
  /**
   * Work around for https://github.com/microsoft/TypeScript/issues/57035
   */
  self() {
    return this;
  }
  /**
   * Export the contents of this definition.
   *
   * This is called internally by the Convex framework.
   * @internal
   */
  export() {
    let t = this.validator.json;
    if (typeof t != "object")
      throw new Error(
        "Invalid validator: please make sure that the parameter of `defineTable` is valid (see https://docs.convex.dev/database/schemas)"
      );
    return {
      indexes: this.indexes,
      stagedDbIndexes: this.stagedDbIndexes,
      searchIndexes: this.searchIndexes,
      stagedSearchIndexes: this.stagedSearchIndexes,
      vectorIndexes: this.vectorIndexes,
      stagedVectorIndexes: this.stagedVectorIndexes,
      documentType: t,
      ...this._monotonicCreationTime && { monotonicCreationTime: !0 }
    };
  }
};
function Ke(e) {
  return _e(e) ? new be(e) : new be(c.object(e));
}
n(Ke, "defineTable");
var Ze = class {
  static {
    n(this, "SchemaDefinition");
  }
  /**
   * @internal
   */
  constructor(t, r) {
    E(this, "tables"), E(this, "strictTableNameTypes"), E(this, "schemaValidation"), this.tables = t, this.schemaValidation = r?.schemaValidation === void 0 ? !0 : r.schemaValidation;
  }
  /**
   * Export the contents of this definition.
   *
   * This is called internally by the Convex framework.
   * @internal
   */
  export() {
    return JSON.stringify({
      tables: Object.entries(this.tables).map(([t, r]) => {
        let {
          indexes: o,
          stagedDbIndexes: s,
          searchIndexes: i,
          stagedSearchIndexes: a,
          vectorIndexes: u,
          stagedVectorIndexes: p,
          documentType: I,
          monotonicCreationTime: F
        } = r.export();
        return {
          tableName: t,
          indexes: o,
          stagedDbIndexes: s,
          searchIndexes: i,
          stagedSearchIndexes: a,
          vectorIndexes: u,
          stagedVectorIndexes: p,
          documentType: I,
          ...F && { monotonicCreationTime: F }
        };
      }),
      schemaValidation: this.schemaValidation
    });
  }
};
function Bt(e, t) {
  return new Ze(e, t);
}
n(Bt, "defineSchema");
var Fs = Bt({
  _scheduled_functions: Ke({
    name: c.string(),
    args: c.array(c.any()),
    scheduledTime: c.float64(),
    completedTime: c.optional(c.float64()),
    state: c.union(
      c.object({ kind: c.literal("pending") }),
      c.object({ kind: c.literal("inProgress") }),
      c.object({ kind: c.literal("success") }),
      c.object({ kind: c.literal("failed"), error: c.string() }),
      c.object({ kind: c.literal("canceled") })
    )
  }),
  _storage: Ke({
    sha256: c.string(),
    size: c.float64(),
    contentType: c.optional(c.string())
  })
});

// ../convex/dist/esm/server/components/index.js
function jt(e, t) {
  let r = {
    get(o, s) {
      if (typeof s == "string") {
        let i = [...t, s];
        return jt(e, i);
      } else if (s === $e) {
        if (t.length < 1) {
          let i = [e, ...t].join(".");
          throw new Error(
            `API path is expected to be of the form \`${e}.childComponent.functionName\`. Found: \`${i}\``
          );
        }
        return "_reference/childComponent/" + t.join("/");
      } else
        return;
    }
  };
  return new Proxy({}, r);
}
n(jt, "createChildComponents");
var nn = /* @__PURE__ */ n(() => jt("components", []), "componentsGeneric");

export {
  c as a,
  ce as b,
  ir as c,
  Rr as d,
  qr as e,
  Br as f,
  jr as g,
  Ur as h,
  Vr as i,
  ws as j,
  Lr as k,
  kr as l,
  Gr as m,
  en as n,
  nn as o,
  Ke as p,
  Bt as q
};
//# sourceMappingURL=3MCB5ZJS.js.map
