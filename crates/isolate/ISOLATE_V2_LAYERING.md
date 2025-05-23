# Isolate v2 Runtime – Layering Overview

Below is a bird-eye "onion map" that shows where Convex's *isolate-v2* code ends and where `rusty-v8` / native V8 begins.  Read it from the **outside in**: the farther right you go, the closer you are to raw V8 internals.

```mermaid
flowchart LR
    %% ───────────────────────────── HOST / TOKIO ─────────────────────────────
    subgraph T["async Rust / Tokio application"]
        direction TB
        A[Incoming request / event] -->|run_isolate_v2_udf()| C1(IsolateThreadClient)
    end

    %% ───────────────────── isolate-v2 RUNTIME (Rust) ────────────────────────
    subgraph V2["isolate-v2 crate"]
        direction TB
        C1 -->|mpsc| THD[V8 Thread (rs)]
        C1[IsolateThreadClient\n• user-timeout accounting\n• semaphore\n• mpsc sender]
        THD --> SES[Session\nHandleScope + callbacks]
        SES --> CTX(Context\nv8::Context + Convex glue)
        CTX --> ENV[Environment\ntrait impl – UdfEnvironment etc.]
        CTX --> MODS[ModuleGraph / Loader]
        CTX --> PEND[Pending promises\nasync ops, imports]
    end

    %% ───────────────────── rusty-v8 SURFACE ─────────────────────
    subgraph RV8["rusty-v8 bindings"]
        THD -- owns --> ISO(v8::OwnedIsolate)
        SES -- borrows --> HS(v8::HandleScope)
        CTX -- wraps --> VCTX(v8::Context)
    end

    %% ───────────────────── NATIVE V8 ENGINE ─────────────────────
    subgraph V8["Google V8 engine"]
        VCTX --> JS[💡 Executing JS / Wasm\n(JIT, GC, etc.)]
    end

    %% helper styling
    ENV <..> CTX
    style MODS fill:#eef,stroke:#77f
    style PEND fill:#ffd,stroke:#aa0
```

### Layer cheat-sheet
1. **Tokio / application layer** – calls `run_isolate_v2_udf`, gets streamed log lines & return value.
2. **isolate-v2 runtime (pure Rust)** – owns the V8 thread, keeps track of contexts, environments, pending async work, logging, RNG & deterministic time.
3. **rusty-v8** – safe(ish) Rust veneers over the C++ V8 API (`OwnedIsolate`, `HandleScope`, …).
4. **Native V8** – executes user JavaScript / WebAssembly.

Solid arrows = synchronous ownership / invocation.  The dashed double arrow between **Context** and **Environment** highlights that syscalls travel both ways (JS ➜ Rust for database access, Rust ➜ JS to resolve Promises).