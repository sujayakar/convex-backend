import { ConvexHttpClient } from "convex/browser";
import { api } from "./convex/_generated/api";
import { deploymentUrl } from "./common";

describe("Monotonic Creation Times", () => {
  let httpClient: ConvexHttpClient;

  beforeEach(async () => {
    httpClient = new ConvexHttpClient(deploymentUrl);
    // Clean up any leftover data from previous runs
    await httpClient.mutation(api.cleanUp.default);
  });

  afterEach(async () => {
    // Use standard cleanup that clears all tables including monotonic_docs
    await httpClient.mutation(api.cleanUp.default);
  });

  test("Documents inserted in same mutation have strictly increasing _creationTime", async () => {
    // Insert 5 documents in a single mutation
    const docs = await httpClient.mutation(
      api.monotonicCreationTime.insertMany,
      { count: 5 },
    );

    expect(docs.length).toBe(5);

    // Verify each document has a _creationTime
    for (const doc of docs) {
      expect(doc).not.toBeNull();
      expect(doc).toHaveProperty("_creationTime");
      expect(typeof doc!._creationTime).toBe("number");
    }

    // Verify creation times are strictly increasing
    for (let i = 1; i < docs.length; i++) {
      expect(docs[i]!._creationTime).toBeGreaterThan(docs[i - 1]!._creationTime);
    }
  });

  test("Documents inserted across separate mutations have monotonically increasing _creationTime", async () => {
    // Insert documents one at a time across multiple mutations
    const doc1 = await httpClient.mutation(
      api.monotonicCreationTime.insertOne,
      { value: 1 },
    );
    const doc2 = await httpClient.mutation(
      api.monotonicCreationTime.insertOne,
      { value: 2 },
    );
    const doc3 = await httpClient.mutation(
      api.monotonicCreationTime.insertOne,
      { value: 3 },
    );

    // Verify creation times are strictly increasing
    expect(doc2!._creationTime).toBeGreaterThan(doc1!._creationTime);
    expect(doc3!._creationTime).toBeGreaterThan(doc2!._creationTime);
  });

  test("Rapid insertions maintain monotonicity", async () => {
    // Fire off multiple mutations as fast as possible
    const promises = [];
    for (let i = 0; i < 10; i++) {
      promises.push(
        httpClient.mutation(api.monotonicCreationTime.insertOne, { value: i }),
      );
    }

    // Wait for all mutations to complete
    await Promise.all(promises);

    // Query all documents ordered by creation time
    const orderedDocs = await httpClient.query(
      api.monotonicCreationTime.listByCreationTime,
    );

    // Verify creation times are all unique (strictly increasing)
    const creationTimes = orderedDocs.map((d) => d._creationTime);
    const uniqueTimes = new Set(creationTimes);
    expect(uniqueTimes.size).toBe(creationTimes.length);

    // Verify they're sorted (monotonically increasing)
    for (let i = 1; i < creationTimes.length; i++) {
      expect(creationTimes[i]).toBeGreaterThan(creationTimes[i - 1]);
    }
  });

  test("Large batch insertion maintains monotonicity", async () => {
    // Insert many documents in a single mutation
    const docs = await httpClient.mutation(
      api.monotonicCreationTime.insertMany,
      { count: 20 },
    );

    expect(docs.length).toBe(20);

    // Verify all creation times are strictly increasing (in insertion order)
    for (let i = 1; i < docs.length; i++) {
      expect(docs[i]!._creationTime).toBeGreaterThan(docs[i - 1]!._creationTime);
    }

    // Verify all creation times are unique
    const creationTimes = docs.map((d) => d!._creationTime);
    const uniqueTimes = new Set(creationTimes);
    expect(uniqueTimes.size).toBe(20);
  });
});
