# Integration Tests

These tests use the `ConvexHttpClient` and `ConvexReactClient` to talk to a real
backend.

## Run against a local backend (convex-local-backend)

To run tests against a backend you start yourself (e.g. for working on backend
or integration tests without the full harness):

1. **Start the backend** (from repo root):
   ```sh
   just run-local-backend
   ```
   Or: `cargo run -p local_backend --bin convex-local-backend` (listens on
   port 3210 by default).

2. **Build this package and its deps** (from repo root, once):
   ```sh
   just rush build -t js-integration-tests
   ```

3. **Deploy and run tests** (from this directory, `npm-packages/js-integration-tests`):
   ```sh
   just test-local-backend
   ```
   To run a single test file:
   ```sh
   just test-local-backend monotonic_creation_time.test.ts
   ```

You can also set `DEPLOYMENT_URL`, `SITE_URL`, and `ADMIN_KEY` yourself and run
`npm run test-integration`; `common.ts` uses the dev admin key from
`crates/keybroker/dev/admin_key.txt` when those are not set.

## Run with backend harness (CI / full stack)

Run `just test` from this directory to run a rush build, and then run
integration tests.

Once that's done, you can run `just _test` during subsequent iterations if
you're only modifying the test suite. This will speed things up as it does not
rerun the rush build, it only spins up a backend and re-runs the test suite.

## Run individual test files

```sh
just test someFile.test.ts
just _test conductor-debug someFile.test.ts
```

Remember that your file name needs to end in `.test.ts` or `.test.tsx`.

## State and Concurrency

Because all of these tests run against the same backend, there is a large risk
of leaking state between tests.

To solve this we:

1. Set Jest's `maxWorkers` to 1 so only 1 test runs at a time.
2. Have a `cleanUp` mutation that deletes all data after each test.

To ensure that `cleanUp` is complete, make sure to:

1. Add it to every new suite.
2. Put all new table names in `schema.ts:ALL_TABLE_NAMES` so we clear the table
   after every test.
