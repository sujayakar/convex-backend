import {
  a as i,
  c as Q
} from "./U5LILI2V.js";

// ../common/temp/node_modules/.pnpm/check-error@2.1.1/node_modules/check-error/index.js
var v = {};
Q(v, {
  compatibleConstructor: () => H,
  compatibleInstance: () => F,
  compatibleMessage: () => J,
  getConstructorName: () => z,
  getMessage: () => G
});
function N(o) {
  return o instanceof Error || Object.prototype.toString.call(o) === "[object Error]";
}
i(N, "isErrorInstance");
function W(o) {
  return Object.prototype.toString.call(o) === "[object RegExp]";
}
i(W, "isRegExp");
function F(o, n) {
  return N(n) && o === n;
}
i(F, "compatibleInstance");
function H(o, n) {
  return N(n) ? o.constructor === n.constructor || o instanceof n.constructor : (typeof n == "object" || typeof n == "function") && n.prototype ? o.constructor === n || o instanceof n : !1;
}
i(H, "compatibleConstructor");
function J(o, n) {
  let c = typeof o == "string" ? o : o.message;
  return W(n) ? n.test(c) : typeof n == "string" ? c.indexOf(n) !== -1 : !1;
}
i(J, "compatibleMessage");
function z(o) {
  let n = o;
  return N(o) ? n = o.constructor.name : typeof o == "function" && (n = o.name, n === "" && (n = new o().name || n)), n;
}
i(z, "getConstructorName");
function G(o) {
  let n = "";
  return o && o.message ? n = o.message : typeof o == "string" && (n = o), n;
}
i(G, "getMessage");

// ../common/temp/node_modules/.pnpm/chai-as-promised@8.0.2_chai@4.5.0/node_modules/chai-as-promised/lib/chai-as-promised.js
var l = v;
function K(o, n) {
  let c = o.Assertion, r = o.assert, C = n.proxify;
  n.checkError && (l = n.checkError);
  function $(t) {
    return typeof t.catch != "function" && typeof t.always == "function" && typeof t.done == "function" && typeof t.fail == "function" && typeof t.pipe == "function" && typeof t.progress == "function" && typeof t.state == "function";
  }
  i($, "isLegacyJQueryPromise");
  function O(t) {
    if (typeof t._obj.then != "function")
      throw new TypeError(
        n.inspect(t._obj) + " is not a thenable."
      );
    if ($(t._obj))
      throw new TypeError(
        "Chai as Promised is incompatible with thenables of jQuery<3.0.0, sorry! Please upgrade jQuery or use another Promises/A+ compatible library (see http://promisesaplus.com/)."
      );
  }
  i(O, "assertIsAboutPromise");
  function T(t) {
    return C === void 0 ? t : C(t);
  }
  i(T, "proxifyIfSupported");
  function w(t, e) {
    n.addMethod(c.prototype, t, function() {
      return O(this), e.apply(this, arguments);
    });
  }
  i(w, "method");
  function j(t, e) {
    n.addProperty(c.prototype, t, function() {
      return O(this), T(e.apply(this, arguments));
    });
  }
  i(j, "property");
  function _(t, e) {
    t.then(() => e(), e);
  }
  i(_, "doNotify");
  function I(t, e, s) {
    t.assert(!0, null, e, s.expected, s.actual);
  }
  i(I, "assertIfNegated");
  function x(t, e, s) {
    t.assert(!1, e, null, s.expected, s.actual);
  }
  i(x, "assertIfNotNegated");
  function y(t) {
    return typeof t.then == "function" ? t : t._obj;
  }
  i(y, "getBasePromise");
  function E(t) {
    return t instanceof Error ? t.toString() : l.getConstructorName(t);
  }
  i(E, "getReasonName");
  let A = Object.getOwnPropertyNames(c.prototype), P = {};
  for (let t of A)
    P[t] = Object.getOwnPropertyDescriptor(
      c.prototype,
      t
    );
  j("fulfilled", function() {
    let t = y(this).then(
      (e) => (I(
        this,
        "expected promise not to be fulfilled but it was fulfilled with #{act}",
        { actual: e }
      ), e),
      (e) => (x(
        this,
        "expected promise to be fulfilled but it was rejected with #{act}",
        { actual: E(e) }
      ), e)
    );
    return g(this, t), this;
  }), j("rejected", function() {
    let t = y(this).then(
      (e) => (x(
        this,
        "expected promise to be rejected but it was fulfilled with #{act}",
        { actual: e }
      ), e),
      (e) => (I(
        this,
        "expected promise not to be rejected but it was rejected with #{act}",
        { actual: E(e) }
      ), e)
    );
    return g(this, t), this;
  }), w("rejectedWith", function(t, e, s) {
    let f = null, a = n.flag(this, "negate") || !1;
    if (t === void 0 && e === void 0 && s === void 0)
      return this.rejected;
    s !== void 0 && n.flag(this, "message", s), t instanceof RegExp || typeof t == "string" ? (e = t, t = null) : t && t instanceof Error ? f = t.toString() : typeof t == "function" ? f = l.getConstructorName(t) : t = null;
    let h = !!(t && e), p = "including";
    e instanceof RegExp && (p = "matching");
    let B = y(this).then(
      (u) => {
        let d = null, m = null;
        return t ? (d = "expected promise to be rejected with #{exp} but it was fulfilled with #{act}", m = f) : e && (d = `expected promise to be rejected with an error ${p} #{exp} but it was fulfilled with #{act}`, m = e), x(this, d, { expected: m, actual: u }), u;
      },
      (u) => {
        let d = t && (t instanceof Error ? l.compatibleInstance(u, t) : l.compatibleConstructor(u, t)), m = e === u || e && u && l.compatibleMessage(u, e), R = E(u);
        return a && h ? d && m && this.assert(
          !0,
          null,
          "expected promise not to be rejected with #{exp} but it was rejected with #{act}",
          f,
          R
        ) : (t && this.assert(
          d,
          "expected promise to be rejected with #{exp} but it was rejected with #{act}",
          "expected promise not to be rejected with #{exp} but it was rejected with #{act}",
          f,
          R
        ), e && this.assert(
          m,
          `expected promise to be rejected with an error ${p} #{exp} but got #{act}`,
          `expected promise not to be rejected with an error ${p} #{exp}`,
          e,
          l.getMessage(u)
        )), u;
      }
    );
    return g(this, B), this;
  }), j("eventually", function() {
    return n.flag(this, "eventually", !0), this;
  }), w("notify", function(t) {
    return _(y(this), t), this;
  }), w("become", function(t, e) {
    return this.eventually.deep.equal(t, e);
  }), A.filter((t) => t !== "assert" && typeof P[t].value == "function").forEach((t) => {
    c.overwriteMethod(
      t,
      (e) => function() {
        return b(e, this, arguments);
      }
    );
  }), A.filter((t) => t !== "_obj" && typeof P[t].get == "function").forEach((t) => {
    Object.prototype.hasOwnProperty.call(
      c.prototype.__methods,
      t
    ) ? c.overwriteChainableMethod(
      t,
      (s) => function() {
        return b(s, this, arguments);
      },
      (s) => function() {
        return b(s, this);
      }
    ) : c.overwriteProperty(
      t,
      (s) => function() {
        return T(
          b(s, this)
        );
      }
    );
  });
  function b(t, e, s) {
    if (!n.flag(e, "eventually"))
      return t.apply(e, s), e;
    let f = y(e).then((a) => (e._obj = a, n.flag(e, "eventually", !1), s && V(s))).then((a) => (t.apply(e, a), e._obj));
    return g(e, f), e;
  }
  i(b, "doAsserterAsyncAndAddThen");
  let q = Object.getOwnPropertyNames(r).filter(
    (t) => typeof r[t] == "function"
  );
  r.isFulfilled = (t, e) => new c(t, e).to.be.fulfilled, r.isRejected = (t, e, s, f) => new c(t, f).to.be.rejectedWith(e, s, f), r.becomes = (t, e, s) => r.eventually.deepEqual(t, e, s), r.doesNotBecome = (t, e, s) => r.eventually.notDeepEqual(t, e, s), r.eventually = {}, q.forEach((t) => {
    r.eventually[t] = function(e) {
      let s = Array.prototype.slice.call(arguments, 1), f, a = arguments[r[t].length - 1];
      typeof a == "string" && (f = /* @__PURE__ */ i((p) => {
        throw new o.AssertionError(
          `${a}

Original reason: ${n.inspect(p)}`
        );
      }, "customRejectionHandler"));
      let h = e.then(
        (p) => r[t].apply(
          r,
          [p].concat(s)
        ),
        f
      );
      return h.notify = (p) => {
        _(h, p);
      }, h;
    };
  });
}
i(K, "default");
function U(o, n) {
  o.then = n.then.bind(n);
}
i(U, "defaultTransferPromiseness");
var S, D;
function g(o, n) {
  S ? S(o, n) : U(o, n);
}
i(g, "transferPromiseness");
function V(o) {
  return D ? D(o) : o;
}
i(V, "transformAsserterArgs");

export {
  K as a
};
//# sourceMappingURL=RKEXAGFN.js.map
