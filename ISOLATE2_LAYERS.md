# isolate2 Layering Diagram

```mermaid
flowchart TD
  %% ─────────── outside: the Rust orchestration on a Tokio task ───────────
  subgraph R1["Tokio side (Rust async task)"]
    Run[run_isolate_v2_udf()]
    Client[IsolateThreadClient ⇄ IsolateThreadRequest/Completions]
  end

  %% ─────────── V8 runs on a dedicated OS thread ───────────
  Run --spawns--> V8T[V8 thread • isolate2::runner::v8_thread]

  subgraph R2["V8 thread (Rust ↔︎ V8)"]
    ThreadObj[Thread\nv8::OwnedIsolate]
    SessionObj[Session\nHandleScope + HeapContext]
    Ctx[Context\nv8::Context pool]
    Enter[EnteredContext\n(executing JS)]
    JS[User JS / UDF code]
  end

  %% nesting / layering
  ThreadObj --> SessionObj
  SessionObj --> Ctx
  Ctx --> Enter
  Enter --> JS

  %% host ↔︎ guest boundary
  JS -- calls --> CB[CallbackContext.syscall()/asyncSyscall()/op()]
  CB --delegates--> Env[UdfEnvironment implements<br/>Environment + SyscallProvider]

  %% talking to the platform
  Env -- DB / storage / schedulers --> DB[(Database & platform services)]

  %% async-syscall completions travel back
  Env -.results.-> Client
  Client -.wake-ups.-> CB
```

## Legend

- **Solid arrows** – direct call/ownership chain (outer → inner layer).
- **Dashed arrows** – message passing / callbacks across threads.