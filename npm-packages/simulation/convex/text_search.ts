import { v } from "convex/values";
import { mutation, query } from "./_generated/server";

// Text search scenario: concurrent inserts + full-text search queries.
// Tests search index updates under concurrency and result correctness.

export const insertDocument = mutation({
  args: {
    title: v.string(),
    body: v.string(),
    category: v.string(),
  },
  returns: v.id("searchDocs"),
  handler: async (ctx, args) => {
    return await ctx.db.insert("searchDocs", {
      title: args.title,
      body: args.body,
      category: args.category,
    });
  },
});

export const searchByBody = query({
  args: {
    searchTerm: v.string(),
  },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("searchDocs")
      .withSearchIndex("by_body", (q) => q.search("body", args.searchTerm))
      .collect();
  },
});

export const searchByBodyWithFilter = query({
  args: {
    searchTerm: v.string(),
    category: v.string(),
  },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("searchDocs")
      .withSearchIndex("by_body", (q) =>
        q.search("body", args.searchTerm).eq("category", args.category),
      )
      .collect();
  },
});

export const countAll = query({
  args: {},
  returns: v.number(),
  handler: async (ctx) => {
    const docs = await ctx.db.query("searchDocs").collect();
    return docs.length;
  },
});
