import { mutation, query } from "./_generated/server";
import { v } from "convex/values";

// Insert a single document and return it with _creationTime
export const insertOne = mutation({
  args: { value: v.number() },
  handler: async (ctx, args) => {
    const id = await ctx.db.insert("monotonic_docs", { value: args.value });
    return await ctx.db.get(id);
  },
});

// Insert multiple documents in a single mutation and return them all
export const insertMany = mutation({
  args: { count: v.number() },
  handler: async (ctx, args) => {
    const docs = [];
    for (let i = 0; i < args.count; i++) {
      const id = await ctx.db.insert("monotonic_docs", { value: i });
      const doc = await ctx.db.get(id);
      docs.push(doc);
    }
    return docs;
  },
});

// Query all documents ordered by _creationTime
// Note: The default index orders by _id, but since documents are created
// monotonically, ordering by _creationTime should match insertion order
export const listByCreationTime = query({
  args: {},
  handler: async (ctx) => {
    // Query and sort by _creationTime to get documents in creation order
    const docs = await ctx.db.query("monotonic_docs").collect();
    return docs.sort((a, b) => a._creationTime - b._creationTime);
  },
});

// Clean up all documents in the monotonic_docs table
export const cleanUp = mutation({
  args: {},
  handler: async (ctx) => {
    const docs = await ctx.db.query("monotonic_docs").collect();
    for (const doc of docs) {
      await ctx.db.delete(doc._id);
    }
  },
});
