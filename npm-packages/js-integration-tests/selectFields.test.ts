import { ConvexHttpClient } from "convex/browser";
import { api } from "./convex/_generated/api";
import { deploymentUrl } from "./common";

describe("Field selection", () => {
  let client: ConvexHttpClient;

  beforeEach(() => {
    client = new ConvexHttpClient(deploymentUrl);
  });

  afterEach(async () => {
    await client.mutation(api.cleanUp.default);
  });

  test("select single field", async () => {
    await client.mutation(api.selectFields.insertUser, {
      name: "Alice",
      email: "alice@test.com",
      age: 30,
      bio: "Long bio text...",
    });

    const results = await client.query(api.selectFields.selectNameOnly, {});

    expect(results).toHaveLength(1);
    expect(results[0]).toHaveProperty("_id");
    expect(results[0]).toHaveProperty("_creationTime");
    expect(results[0]).toHaveProperty("name", "Alice");
    expect(results[0]).not.toHaveProperty("email");
    expect(results[0]).not.toHaveProperty("age");
    expect(results[0]).not.toHaveProperty("bio");
  });

  test("select multiple fields", async () => {
    await client.mutation(api.selectFields.insertUser, {
      name: "Bob",
      email: "bob@test.com",
      age: 25,
    });
    await client.mutation(api.selectFields.insertUser, {
      name: "Charlie",
    });

    const results = await client.query(api.selectFields.selectMultipleFields, {});

    expect(results).toHaveLength(2);
    const bob = results.find((result) => result.name === "Bob");
    const charlie = results.find((result) => result.name === "Charlie");
    expect(bob).toBeDefined();
    expect(bob).toHaveProperty("email", "bob@test.com");
    expect(bob).not.toHaveProperty("age");
    expect(charlie).toBeDefined();
    expect(charlie).not.toHaveProperty("email");
  });

  test("select with filter", async () => {
    await client.mutation(api.selectFields.insertUser, {
      name: "Young",
      age: 20,
    });
    await client.mutation(api.selectFields.insertUser, {
      name: "Old",
      age: 40,
    });

    const results = await client.query(api.selectFields.selectWithFilter, {
      minAge: 30,
    });

    expect(results).toHaveLength(1);
    expect(results[0].name).toBe("Old");
    expect(results[0].age).toBe(40);
    expect(results[0]).not.toHaveProperty("email");
  });

  test("select with first()", async () => {
    await client.mutation(api.selectFields.insertUser, { name: "First" });
    await client.mutation(api.selectFields.insertUser, { name: "Second" });

    const result = await client.query(api.selectFields.selectFirst, {});

    expect(result).not.toBeNull();
    expect(result).toHaveProperty("name", "First");
    expect(result).not.toHaveProperty("email");
  });

  test("empty select returns only system fields", async () => {
    await client.mutation(api.selectFields.insertUser, {
      name: "Alice",
      email: "a@t.com",
      age: 30,
    });

    const results = await client.query(api.selectFields.selectEmpty, {});

    expect(results).toHaveLength(1);
    const keys = Object.keys(results[0]).sort();
    expect(keys).toEqual(["_creationTime", "_id"]);
  });

  test("select with index", async () => {
    for (let i = 0; i < 10; i++) {
      await client.mutation(api.selectFields.insertUser, { name: `User${i}` });
    }

    const results = await client.query(api.selectFields.selectWithIndex, {});

    expect(results).toHaveLength(5);
    for (const result of results) {
      expect(result).toHaveProperty("name");
      expect(result).not.toHaveProperty("email");
    }
  });

  test("select with pagination", async () => {
    for (let i = 0; i < 5; i++) {
      await client.mutation(api.selectFields.insertUser, { name: `User${i}` });
    }

    const page1 = await client.query(api.selectFields.selectWithPagination, {
      paginationOpts: { numItems: 2, cursor: null },
    });

    expect(page1.page).toHaveLength(2);
    expect(page1.isDone).toBe(false);
    for (const doc of page1.page) {
      expect(doc).toHaveProperty("name");
      expect(doc).not.toHaveProperty("email");
    }

    const page2 = await client.query(api.selectFields.selectWithPagination, {
      paginationOpts: { numItems: 10, cursor: page1.continueCursor },
    });

    expect(page2.page).toHaveLength(3);
    expect(page2.isDone).toBe(true);
  });
});
