import {
  a as o
} from "./U5LILI2V.js";

// convex/helpers.ts
function u(r) {
  let e = 0, t = 1;
  for (let n = 0; n < r; n++) {
    let s = e + t;
    e = t, t = s;
  }
  return e;
}
o(u, "fibonacci");
function c() {
  throw new Error("Doesn't has an apostrophe.");
}
o(c, "doesntWork");
var i = /* @__PURE__ */ o((r) => new Promise((e) => setTimeout(e, r)), "sleep");

export {
  u as a,
  c as b,
  i as c
};
//# sourceMappingURL=6BRUGAVX.js.map
