import {
  a as o,
  c as a,
  e as s
} from "./_deps/KYTPMCQL.js";
import "./_deps/3MCB5ZJS.js";
import "./_deps/U5LILI2V.js";

// convex/basic.ts
var y = o(async (t, { x: e }) => e + 1n), j = o(async () => (/* @__PURE__ */ new Date()).toString()), p = o(async () => Date.now()), u = o(async (t, { args: e }) => +new Date(...e)), l = o(async ({ db: t }, { id: e }) => t.get(e)), x = a(async ({ db: t }, e) => {
  let n = await t.insert("objects", e);
  return await t.get(n);
}), b = a(async ({ db: t }) => {
  let e = { field: "a" }, n = await t.insert("objects", e);
  e.field = "ab", await t.patch(n, e), await t.delete(n);
}), g = a(
  async ({ db: t }, { obj1: e, obj2: n }) => {
    let r = await t.insert("objects", e), c = await t.insert("objects", n);
    return [await t.get(r), await t.get(c)];
  }
), m = a(
  async ({ db: t }, { id: e, obj: n }) => (await t.patch(e, n), await t.get(e))
), d = a(
  async ({ db: t }, { id: e, fieldName: n }) => {
    let r = {};
    return r[n] = void 0, await t.patch(e, r), await t.get(e);
  }
), h = a(
  async ({ db: t }, { id: e, obj: n }) => (await t.replace(e, n), await t.get(e))
), f = a(async ({ db: t }, e) => {
  let n = await t.insert("objects", e);
  return e = await t.get(n), await t.delete(n), e;
}), O = o(async ({ db: t }) => await t.query("objects").collect()), I = a(async ({ db: t }) => {
  let e = await t.insert("objects", {
    name: "test"
  }), n = await t.get("objects", e);
  if (!n || n.name !== "test")
    throw new Error();
  await t.patch("objects", e, {
    name: "test2"
  });
  let r = await t.get("objects", e);
  if (!r || r.name !== "test2")
    throw new Error();
  await t.replace("objects", e, {
    name: "test3"
  });
  let c = await t.get("objects", e);
  if (!c || c.name !== "test3")
    throw new Error();
  if (await t.delete("objects", e), await t.get("objects", e) !== null)
    throw new Error();
}), A = o(async () => "hi"), D = o(async ({ db: t }) => await t.query("objects").count()), q = a(async ({ db: t }, e) => (await t.insert("objects", e), await t.query("objects").count())), T = a(
  async ({ db: t }, { id: e }) => (await t.delete(e), await t.query("objects").count())
), E = a(async () => 2), M = s(async () => 2);
export {
  y as addOneInt,
  D as count,
  u as createTimeMs,
  T as deleteAndCount,
  d as deleteObjectField,
  A as doNothing,
  I as explicitDbTableApi,
  l as getObject,
  q as insertAndCount,
  f as insertAndDeleteObject,
  b as insertModifyDeleteObject,
  x as insertObject,
  g as insertTwoObjects,
  O as listAllObjects,
  m as patchObject,
  j as readTime,
  p as readTimeMs,
  h as replaceObject,
  M as simpleAction,
  E as simpleMutation
};
//# sourceMappingURL=basic.js.map
