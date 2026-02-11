import {
  a as f
} from "./_deps/F7OEUBKV.js";
import "./_deps/RKEXAGFN.js";
import {
  a as l,
  c as e
} from "./_deps/4ZOYWIIA.js";
import {
  a as w
} from "./_deps/MF64ESCW.js";
import {
  a as u,
  e as h
} from "./_deps/KYTPMCQL.js";
import "./_deps/3MCB5ZJS.js";
import {
  a as o
} from "./_deps/U5LILI2V.js";

// convex/fetch.ts
var gt = u({
  args: {},
  handler: /* @__PURE__ */ o(async () => {
    await fetch("http://localhost:4545/assets/fixture.json");
  }, "handler")
}), Tt = u({
  args: {},
  handler: /* @__PURE__ */ o(async (t) => await t.db.query("triggerAbort").first() !== null, "handler")
});
async function m(t) {
  let a = new AbortController();
  (async () => {
    for (; !await t.runQuery(w.fetch.checkForAbort); )
      await new Promise((n) => setTimeout(n, 10));
    a.abort();
  })();
  let s = fetch("http://localhost:4548/pause", {
    method: "GET",
    signal: a.signal
  });
  await l(s).to.be.rejectedWith("AbortError");
}
o(m, "fetchAbortTest");
var Rt = h({
  args: {},
  handler: /* @__PURE__ */ o(async (t) => await f({
    fetchAbortTest: /* @__PURE__ */ o(() => m(t), "fetchAbortTest")
  }), "handler")
}), vt = h(async () => await f({
  fetchConstructorClones: ft,
  fetchRequiresOneArgument: b,
  fetchProtocolError: x,
  fetchDnsError: E,
  fetchInvalidUriError: q,
  fetchMalformedUriError: g,
  fetchJson: T,
  fetchURL: R,
  fetchHeaders: v,
  fetchBlob: _,
  fetchBodyUsedReader: B,
  // This does not currently pass because we're not tracking this state between
  // the Response and the ReadableStream
  // fetchBodyUsedCancelStream,
  fetchAsyncIterator: S,
  fetchBodyReader: j,
  fetchBodyReaderBigBody: P,
  fetchBodyReaderMultiPartBody: F,
  responseClone: O,
  fetchMultipartFormDataSuccess: U,
  fetchMultipartFormBadContentType: W,
  fetchURLEncodedFormDataSuccess: A,
  fetchInitFormDataBinaryFileBody: k,
  fetchInitFormDataMultipleFilesBody: L,
  fetchWithRedirection: D,
  // fetchWithRelativeRedirection,
  fetchRedirectPostToGet: N,
  fetchWithRelativeRedirectionUrl: H,
  fetchWithInfRedirection: z,
  fetchCrossOriginRedirectStripsAuthorization: I,
  fetchRedirectBody: C,
  // fetchRedirectBodyStream,
  fetchInitStringBody: M,
  fetchRequestInitStringBody: G,
  fetchSeparateInit: J,
  fetchInitTypedArrayBody: K,
  fetchInitArrayBufferBody: Q,
  fetchInitURLSearchParamsBody: X,
  fetchInitBlobBody: V,
  fetchInitFormDataBody: $,
  fetchInitFormDataBlobFilenameBody: Y,
  fetchInitFormDataTextFileBody: Z,
  fetchUserAgent: tt,
  // fetchRequest,
  // fetchRequestAcceptHeaders,
  // fetchPostBodyString,
  // fetchPostBodyTypedArray,
  // fetchUserSetContentLength,
  // fetchUserSetTransferEncoding,
  // fetchWithNonAsciiRedirection,
  fetchWithManualRedirection: et,
  fetchWithErrorRedirection: at,
  responseRedirect: ot,
  responseRedirectTakeURLObjectAsParameter: st,
  responseWithoutBody: nt,
  fetchBodyReadTwice: rt,
  fetchBodyReaderAfterRead: ct,
  fetchBodyReaderWithCancelAndNewReader: it,
  fetchBodyReaderWithReadCancelAndNewReader: lt,
  // fetchResourceCloseAfterStreamCancel,
  // fetchNullBodyStatus,
  fetchResponseContentLength: ht,
  // fetchResponseConstructorNullBody,
  // fetchResponseConstructorInvalidStatus,
  // fetchResponseEmptyConstructor,
  // fetchCustomHttpClientParamCertificateSuccess,
  // fetchPostBodyReadableStream,
  // fetchWritableRespProps,
  // fetchFilterOutCustomHostHeader
  // fetchHeadRespBody,
  // fetchClientCertWrongPrivateKey,
  // fetchClientCertBadPrivateKey,
  // fetchClientCertNotPrivateKey,
  // fetchCustomClientPrivateKey,
  // fetchAbortWhileUploadStreaming,
  // fetchAbortWhileUploadStreamingWithReason,
  // fetchAbortWhileUploadStreamingWithPrimitiveReason,
  // fetchHeaderValueShouldNotPanic,
  // fetchHeaderNameShouldNotPanic,
  // fetchSupportsHttp1Only,
  // fetchSupportsHttp2,
  // fetchPrefersHttp2,
  // fetchFilePerm,
  // fetchFilePermDoesNotExist,
  // fetchFileBadMethod,
  // fetchFileDoesNotExist,
  // fetchFile,
  // fetchContentLengthPost,
  // fetchContentLengthPut,
  // fetchContentLengthPatch,
  // fetchContentLengthPostWithStringBody,
  // fetchContentLengthPostWithBufferBody,
  // staticResponseJson,
  // fetchWithInvalidContentLengthAndTransferEncoding,
  // fetchWithInvalidContentLength,
  // fetchWithInvalidContentLength2,
  // fetchBlobUrl,
  fetchResponseStreamIsLockedWhileReading: dt,
  fetchResponseStreamIsLockedWhileReadingBlob: ut,
  fetchForbidden: pt,
  fetchOlaf: yt,
  fetchBodyTextDecoderStream: wt
}));
async function b() {
  await l(
    // @ts-expect-error intentionally pass no arguments
    fetch()
  ).to.be.rejectedWith(TypeError, "Request URL is undefined");
}
o(b, "fetchRequiresOneArgument");
async function x() {
  await l(fetch("ftp://localhost:21/a/file")).to.be.rejectedWith(
    TypeError,
    "Unsupported URL scheme"
  );
}
o(x, "fetchProtocolError");
async function E() {
  await l(fetch("http://invalid/")).to.be.rejectedWith(
    /dns error: failed to lookup address information/
  );
}
o(E, "fetchDnsError");
async function q() {
  await l(fetch("http://<invalid>/")).to.be.rejectedWith(/Invalid URL/);
}
o(q, "fetchInvalidUriError");
async function g() {
  let t = new URL("http://{{google/");
  await l(fetch(t)).to.be.rejectedWith(/Parsed Url is not a valid Uri/);
}
o(g, "fetchMalformedUriError");
async function T() {
  let t = await fetch("http://localhost:4545/assets/fixture.json");
  e.strictEqual(t.ok, !0), e.strictEqual(t.status, 200), e.strictEqual(t.statusText, "OK"), e.strictEqual(t.type, "basic");
  let a = await t.json();
  e.strictEqual(a.name, "convex");
}
o(T, "fetchJson");
async function R() {
  let t = await fetch(
    new URL("http://localhost:4545/assets/fixture.json")
  );
  e.strictEqual(t.url, "http://localhost:4545/assets/fixture.json");
  let a = await t.json();
}
o(R, "fetchURL");
async function v() {
  let t = await fetch("http://localhost:4545/assets/fixture.json"), a = t.headers;
  e.strictEqual(a.get("Content-Type"), "application/json");
  let s = await t.json();
}
o(v, "fetchHeaders");
async function _() {
  let t = await fetch("http://localhost:4545/assets/fixture.json"), a = t.headers, s = await t.blob();
  e.strictEqual(s.type, a.get("Content-Type")), e.strictEqual(s.size, Number(a.get("Content-Length"))), e.strictEqual(await s.text(), '{"name":"convex"}');
}
o(_, "fetchBlob");
async function B() {
  let t = await fetch("http://localhost:4545/assets/fixture.json");
  e(t.body !== null);
  let a = t.body.getReader();
  e.strictEqual(t.bodyUsed, !1), a.releaseLock(), await t.json(), e.strictEqual(t.bodyUsed, !0);
}
o(B, "fetchBodyUsedReader");
async function S() {
  let t = await fetch("http://localhost:4545/assets/fixture.json"), a = t.headers;
  e(t.body !== null);
  let s = 0;
  for await (let n of t.body)
    e(n instanceof Uint8Array), s += n.length;
  e.strictEqual(s, Number(a.get("Content-Length")));
}
o(S, "fetchAsyncIterator");
async function j() {
  let t = await fetch("http://localhost:4545/assets/fixture.json"), a = t.headers;
  e(t.body !== null);
  let s = t.body.getReader(), n = 0;
  for (; ; ) {
    let { done: r, value: c } = await s.read();
    if (r) break;
    e(c), e(c instanceof Uint8Array), n += c.length;
  }
  e.strictEqual(n, Number(a.get("Content-Length")));
}
o(j, "fetchBodyReader");
async function P() {
  let t = "a".repeat(10240), a = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  });
  e(a.body !== null);
  let s = await a.body.getReader(), n = 0;
  for (; ; ) {
    let { done: r, value: c } = await s.read();
    if (r) break;
    e(c), n += c.length;
  }
  e.strictEqual(n, t.length);
}
o(P, "fetchBodyReaderBigBody");
async function F() {
  let t = new TextEncoder(), a = new TextDecoder(), { readable: s, writable: n } = new TransformStream(), r = n.getWriter(), c = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: s
  });
  e(c.ok), e(c.body !== null);
  let i = c.body.getReader();
  await r.write(t.encode("Hello "));
  let d = await i.read();
  e.strictEqual(a.decode(d.value), "Hello "), e.strictEqual(d.done, !1), await r.write(t.encode("World!"));
  let p = await i.read();
  e.strictEqual(a.decode(p.value), "World!"), e.strictEqual(p.done, !1), await r.close();
  let y = await i.read();
  e.strictEqual(y.value, void 0), e.strictEqual(y.done, !0);
}
o(F, "fetchBodyReaderMultiPartBody");
async function O() {
  let t = await fetch("http://localhost:4545/assets/fixture.json"), a = t.clone();
  e(t !== a), e.strictEqual(t.status, a.status), e.strictEqual(t.statusText, a.statusText);
  let s = await t.arrayBuffer(), n = await a.arrayBuffer();
  e.deepEqual(s, n);
}
o(O, "responseClone");
async function U() {
  let a = await (await fetch("http://localhost:4545/multipart_form_data.txt")).formData();
  e(a.has("field_1")), e.strictEqual(a.get("field_1").toString(), `value_1 \r
`), e(a.has("field_2"));
  let s = a.get("field_2");
  e.strictEqual(s.name, "file.js"), e.strictEqual(await s.text(), 'console.log("Hi")');
}
o(U, "fetchMultipartFormDataSuccess");
async function W() {
  let t = await fetch(
    "http://localhost:4545/multipart_form_bad_content_type"
  );
  await l(t.formData()).to.be.rejectedWith(
    TypeError,
    "Body cannot be decoded as form data"
  );
}
o(W, "fetchMultipartFormBadContentType");
async function A() {
  let a = await (await fetch(
    "http://localhost:4545/subdir/form_urlencoded.txt"
  )).formData();
  e(a.has("field_1")), e.strictEqual(a.get("field_1").toString(), "Hi"), e(a.has("field_2")), e.strictEqual(a.get("field_2").toString(), "<Convex>");
}
o(A, "fetchURLEncodedFormDataSuccess");
async function k() {
  let t = new Uint8Array([
    108,
    2,
    0,
    0,
    145,
    22,
    162,
    61,
    157,
    227,
    166,
    77,
    138,
    75,
    180,
    56,
    119,
    188,
    177,
    183
  ]), n = (await (await fetch("http://localhost:4545/echo_multipart_file", {
    method: "POST",
    body: t
  })).formData()).get("file");
  e.strictEqual(n.type, "application/octet-stream"), e.strictEqual(n.name, "file.bin"), e.deepEqual(new Uint8Array(await n.arrayBuffer()), t);
}
o(k, "fetchInitFormDataBinaryFileBody");
async function L() {
  let t = [
    {
      content: new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10, 137, 1, 25]),
      type: "image/png",
      name: "image",
      fileName: "some-image.png"
    },
    {
      content: new Uint8Array([
        108,
        2,
        0,
        0,
        145,
        22,
        162,
        61,
        157,
        227,
        166,
        77,
        138,
        75,
        180,
        56,
        119,
        188,
        177,
        183
      ]),
      name: "file",
      fileName: "file.bin",
      expectedType: "application/octet-stream"
    },
    {
      content: new TextEncoder().encode("deno land"),
      type: "text/plain",
      name: "text",
      fileName: "deno.txt"
    }
  ], a = new FormData();
  a.append("field", "value");
  for (let r of t)
    a.append(
      r.name,
      new Blob([r.content], { type: r.type }),
      r.fileName
    );
  let n = await (await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: a
  })).formData();
  e.strictEqual(a.get("field"), n.get("field"));
  for (let r of t) {
    let c = a.get(r.name), i = n.get(r.name);
    e.strictEqual(c.size, i.size), e.strictEqual(c.name, i.name), e.strictEqual(r.expectedType || r.type, i.type), e.deepEqual(
      new Uint8Array(await i.arrayBuffer()),
      r.content
    );
  }
}
o(L, "fetchInitFormDataMultipleFilesBody");
async function D() {
  let t = await fetch("http://localhost:4545/assets/hello.txt");
  e.strictEqual(t.status, 200), e.strictEqual(t.statusText, "OK"), e.strictEqual(t.url, "http://localhost:4545/assets/fixture.json");
  let a = await t.text();
  e.strictEqual(a, '{"name":"convex"}');
}
o(D, "fetchWithRedirection");
async function I() {
  let t = {
    authorization: "Bearer convex-admin",
    "x-location": "http://localhost:4547/print_auth"
  }, a = await fetch("http://localhost:4547/print_auth", {
    headers: t
  });
  e.strictEqual(a.status, 200);
  let s = await a.text();
  e.strictEqual(s, '{"auth":"Bearer convex-admin"}');
  let n = await fetch("http://localhost:4545/assets/hello.txt", {
    headers: t
  });
  e.strictEqual(n.status, 200), e.strictEqual(n.url, "http://localhost:4547/print_auth");
  let r = await n.text();
  e.strictEqual(r, '{"auth":"None"}');
}
o(I, "fetchCrossOriginRedirectStripsAuthorization");
async function C() {
  let t = await fetch("http://localhost:4545/redirect_body", {
    method: "POST",
    body: "Should be forwarded"
  });
  e.strictEqual(t.status, 200), e.strictEqual(t.url, "http://localhost:4545/echo_server");
  let a = await t.text();
  e.strictEqual(a, "Should be forwarded");
}
o(C, "fetchRedirectBody");
async function N() {
  let t = await fetch("http://localhost:4545/post_redirect_to_get", {
    method: "POST",
    headers: { "x-location": "/assets/fixture.json" }
  });
  e.strictEqual(t.status, 200), e.strictEqual(t.url, "http://localhost:4545/assets/fixture.json");
  let a = await t.text();
  e(a.includes('{"name":"convex"}'));
}
o(N, "fetchRedirectPostToGet");
async function H() {
  let t = [
    ["end", "http://localhost:4545/a/b/end"],
    ["/end", "http://localhost:4545/end"]
  ];
  for (let [a, s] of t) {
    let n = await fetch("http://localhost:4545/a/b/c", {
      headers: new Headers([["x-location", a]])
    });
    e.strictEqual(n.url, s), e.strictEqual(n.redirected, !0), e.strictEqual(n.status, 404), e.strictEqual(await n.text(), "");
  }
}
o(H, "fetchWithRelativeRedirectionUrl");
async function z() {
  await l(
    fetch("http://localhost:4545/assets/hello.txt", {
      headers: { "x-location": "/assets/hello.txt" }
    })
  ).to.be.rejectedWith(TypeError, "redirect");
}
o(z, "fetchWithInfRedirection");
async function M() {
  let t = "Hello World", a = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  }), s = await a.text();
  e.strictEqual(s, t), e(a.headers.get("content-type").startsWith("text/plain"));
}
o(M, "fetchInitStringBody");
async function G() {
  let t = "Hello World", a = new Request("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  }), n = await (await fetch(a)).text();
  e.strictEqual(n, t);
}
o(G, "fetchRequestInitStringBody");
async function J() {
  let t = new Request("http://localhost:4545/assets/fixture.json"), a = {
    method: "GET"
  };
  t.headers.set("foo", "bar");
  let s = await fetch(t, a);
  e.strictEqual(s.status, 200), await s.text();
}
o(J, "fetchSeparateInit");
async function K() {
  let t = "Hello World", s = await (await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: new TextEncoder().encode(t)
  })).text();
  e.strictEqual(s, t);
}
o(K, "fetchInitTypedArrayBody");
async function Q() {
  let t = "Hello World", s = await (await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: new TextEncoder().encode(t).buffer
  })).text();
  e.strictEqual(s, t);
}
o(Q, "fetchInitArrayBufferBody");
async function X() {
  let t = "param1=value1&param2=value2", a = new URLSearchParams(t), s = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: a
  }), n = await s.text();
  e.strictEqual(n, t), e(
    s.headers.get("content-type").startsWith("application/x-www-form-urlencoded")
  );
}
o(X, "fetchInitURLSearchParamsBody");
async function V() {
  let t = "const a = 1", a = new Blob([t], {
    type: "text/javascript"
  }), s = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: a
  }), n = await s.text();
  e.strictEqual(n, t), e(s.headers.get("content-type").startsWith("text/javascript"));
}
o(V, "fetchInitBlobBody");
async function $() {
  let t = new FormData();
  t.append("field", "value");
  let s = await (await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  })).formData();
  e.strictEqual(t.get("field"), s.get("field"));
}
o($, "fetchInitFormDataBody");
async function Y() {
  let t = new FormData();
  t.append("field", "value"), t.append("file", new Blob([new TextEncoder().encode("convex")]));
  let s = await (await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  })).formData();
  e.strictEqual(t.get("field"), s.get("field"));
  let n = s.get("file");
  e(n instanceof File), e.strictEqual(n.name, "blob");
}
o(Y, "fetchInitFormDataBlobFilenameBody");
async function Z() {
  let t = "friends of convex", a = new FormData();
  a.append("field", "value"), a.append(
    "file",
    new Blob([new TextEncoder().encode(t)], {
      type: "text/plain"
    }),
    "convex.txt"
  );
  let n = await (await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: a
  })).formData();
  e.strictEqual(a.get("field"), n.get("field"));
  let r = a.get("file"), c = n.get("file");
  e.strictEqual(r.size, c.size), e.strictEqual(r.name, c.name), e.strictEqual(r.type, c.type), e.strictEqual(await r.text(), await c.text());
}
o(Z, "fetchInitFormDataTextFileBody");
async function tt() {
  let a = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: new TextEncoder().encode("Hello World")
  });
  e.equal(a.headers.get("user-agent"), "Convex/1.0"), await a.text();
}
o(tt, "fetchUserAgent");
async function et() {
  let t = await fetch("http://localhost:4545/assets/hello.txt", {
    redirect: "manual"
  });
  e.strictEqual(t.status, 301), e.strictEqual(t.statusText, "Moved Permanently"), e.strictEqual(t.url, "http://localhost:4545/assets/hello.txt"), e.strictEqual(t.type, "basic"), e.strictEqual(t.headers.get("Location"), "/assets/fixture.json"), await t.body.cancel();
}
o(et, "fetchWithManualRedirection");
async function at() {
  await l(
    fetch("http://localhost:4545/assets/hello.txt", {
      redirect: "error"
    })
  ).to.be.rejectedWith(TypeError, "redirect");
}
o(at, "fetchWithErrorRedirection");
function ot() {
  let t = Response.redirect("http://example.com/newLocation", 301);
  e.strictEqual(t.status, 301), e.strictEqual(t.statusText, ""), e.strictEqual(t.url, ""), e.strictEqual(
    t.headers.get("Location"),
    "http://example.com/newLocation"
  ), e.strictEqual(t.type, "default");
}
o(ot, "responseRedirect");
function st() {
  let t = Response.redirect(new URL("https://example.com/"));
  e.strictEqual(t.headers.get("Location"), "https://example.com/");
}
o(st, "responseRedirectTakeURLObjectAsParameter");
async function nt() {
  let t = new Response();
  e.deepEqual(await t.arrayBuffer(), new ArrayBuffer(0));
  let a = await t.blob();
  e.strictEqual(a.size, 0), e.deepEqual(await a.arrayBuffer(), new ArrayBuffer(0)), e.strictEqual(await t.text(), ""), await l(t.json()).to.be.rejectedWith(
    "Unexpected end of JSON input"
  );
}
o(nt, "responseWithoutBody");
async function rt() {
  let t = await fetch("http://localhost:4545/assets/fixture.json"), a = await t.json();
  e(a);
  let s = [
    "json",
    "text",
    /*"formData",*/
    "arrayBuffer"
  ];
  for (let n of s)
    try {
      await t[n](), e(
        !1,
        "Reading body multiple times should failed, the stream should've been locked."
      );
    } catch {
    }
}
o(rt, "fetchBodyReadTwice");
async function ct() {
  let t = await fetch("http://localhost:4545/assets/fixture.json");
  e(t.body !== null);
  let a = await t.body.getReader();
  for (; ; ) {
    let { done: s, value: n } = await a.read();
    if (s) break;
    e(n);
  }
  e.throws(
    () => t.body.getReader(),
    "This stream has already been locked"
  );
}
o(ct, "fetchBodyReaderAfterRead");
async function it() {
  let t = "a".repeat(1024), a = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  });
  e(a.body !== null), await (await a.body.getReader()).releaseLock();
  let n = await a.body.getReader(), r = 0;
  for (; ; ) {
    let { done: c, value: i } = await n.read();
    if (c) break;
    e(i), r += i.length;
  }
  e.strictEqual(r, t.length);
}
o(it, "fetchBodyReaderWithCancelAndNewReader");
async function lt() {
  let t = "a".repeat(1024), a = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  });
  e(a.body !== null);
  let s = await a.body.getReader(), { value: n } = await s.read();
  e(n), await s.releaseLock();
  let r = await a.body.getReader(), c = n.length || 0;
  for (; ; ) {
    let { done: i, value: d } = await r.read();
    if (i) break;
    e(d), c += d.length;
  }
  e.strictEqual(c, t.length);
}
o(lt, "fetchBodyReaderWithReadCancelAndNewReader");
async function ht() {
  let t = new Uint8Array(65536), a = new Headers([["content-type", "application/octet-stream"]]), s = await fetch("http://localhost:4545/echo_server", {
    body: t,
    method: "POST",
    headers: a
  });
  e.strictEqual(
    Number(s.headers.get("content-length")),
    t.byteLength
  );
  let n = await s.blob();
  e.strictEqual(n.type, "application/octet-stream"), e.strictEqual(n.size, t.byteLength);
}
o(ht, "fetchResponseContentLength");
async function dt() {
  let t = await fetch("http://localhost:4545/echo_server", {
    body: new Uint8Array(5e3),
    method: "POST"
  });
  e.strictEqual(t.body.locked, !1);
  let a = t.arrayBuffer();
  e.strictEqual(t.body.locked, !0), await a;
}
o(dt, "fetchResponseStreamIsLockedWhileReading");
async function ut() {
  let t = await fetch("http://localhost:4545/echo_server", {
    body: new Uint8Array(5e3),
    method: "POST"
  });
  e.strictEqual(t.body.locked, !1);
  let a = t.blob();
  e.strictEqual(t.body.locked, !0), await a;
}
o(ut, "fetchResponseStreamIsLockedWhileReadingBlob");
async function ft() {
  let t = new Request("https://example.com", {
    method: "POST",
    body: "foo"
  });
  e.strictEqual(await t.text(), "foo"), await l(t.text()).to.be.rejectedWith(
    TypeError,
    /body stream already read/
  );
  let a = new Request(t, { method: "PUT", body: "bar" });
  e.strictEqual(await a.text(), "bar"), await l(a.text()).to.be.rejectedWith(
    TypeError,
    /body stream already read/
  ), e.strictEqual(t.method, "POST"), e.strictEqual(a.method, "PUT"), e.strictEqual(t.headers.get("x-foo"), null), e.strictEqual(a.headers.get("x-foo"), null), a.headers.set("x-foo", "bar"), e.strictEqual(t.headers.get("x-foo"), null), e.strictEqual(a.headers.get("x-foo"), "bar");
}
o(ft, "fetchConstructorClones");
async function pt() {
  await l(fetch("http://localhost:4545/proxy_reject")).to.be.rejectedWith(
    "Request to http://localhost:4545/proxy_reject forbidden"
  );
}
o(pt, "fetchForbidden");
var _t = h({
  args: {},
  handler: /* @__PURE__ */ o(async () => {
    let t = [
      fetch("http://localhost:4546/timeout"),
      fetch("http://localhost:4546/echo_server", {
        method: "POST",
        body: new TextEncoder().encode("hello world")
      })
    ], a = await Promise.race(t);
    e(a.ok, await a.text());
  }, "handler")
}), Bt = h({
  args: {},
  handler: /* @__PURE__ */ o(async () => {
    let t = /* @__PURE__ */ o(async () => {
      setTimeout(
        () => {
        },
        6e5
      ), await fetch("http://localhost:4546/echo_server", {
        method: "POST",
        body: new TextEncoder().encode("hello world")
      });
    }, "fetchWithTimeout");
    await Promise.all(
      Array(10).fill(0).map(() => t())
    );
  }, "handler")
}), St = h({
  args: {},
  handler: /* @__PURE__ */ o(() => {
    fetch("http://localhost:4546/echo_server");
  }, "handler")
}), jt = h({
  args: {},
  handler: /* @__PURE__ */ o(async () => {
    let t = new Request("http://localhost:4546/timeout"), a = await fetch(t);
    throw new Error(`fetch should not complete: ${a}`);
  }, "handler")
}), Pt = h({
  args: {},
  handler: /* @__PURE__ */ o(async () => {
    let t = new Request("http://localhost:4546/echo_server", {
      method: "POST",
      body: new ReadableStream()
    });
    throw await (await fetch(t)).text(), new Error("fetch should not complete");
  }, "handler")
});
async function yt() {
  let t = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    // hacky way to create a UTF-8 encoded byte string
    headers: { "X-Olaf": unescape(encodeURIComponent("\u26C4")) }
  });
  e.strictEqual(t.headers.get("X-Olaf"), "\xE2\x9B\x84");
}
o(yt, "fetchOlaf");
async function wt() {
  let t = JSON.stringify({
    hello: "world",
    foo: "bar",
    baz: "qux"
  }), a = await fetch("http://localhost:4545/echo_server", {
    method: "POST",
    body: t
  });
  e(a.body !== null);
  let n = a.body.pipeThrough(new TextDecoderStream()).getReader(), r = "";
  for (; ; ) {
    let { done: c, value: i } = await n.read();
    if (c) break;
    e(i), r += i;
  }
  e.strictEqual(r, t);
}
o(wt, "fetchBodyTextDecoderStream");
export {
  Tt as checkForAbort,
  St as danglingFetch,
  vt as default,
  Rt as fetchAbort,
  Bt as fetchBlockedOnTimeouts,
  _t as fetchInParallel,
  jt as fetchTimeout,
  Pt as fetchUnendingRequest,
  gt as fromQuery
};
//# sourceMappingURL=fetch.js.map
