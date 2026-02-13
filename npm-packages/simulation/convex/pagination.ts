import { v } from "convex/values";
import { mutation, query } from "./_generated/server";
import { paginationOptsValidator } from "convex/server";

// Pagination scenario: concurrent inserts while paginating through results.
// Tests cursor stability and that pagination yields a complete, consistent view.

export const insertItem = mutation({
  args: {
    value: v.number(),
    category: v.string(),
  },
  returns: v.id("paginatedItems"),
  handler: async (ctx, args) => {
    return await ctx.db.insert("paginatedItems", {
      value: args.value,
      category: args.category,
    });
  },
});

export const paginateByCategory = query({
  args: {
    category: v.string(),
    paginationOpts: paginationOptsValidator,
  },
  handler: async (ctx, args) => {
    return await ctx.db
      .query("paginatedItems")
      .withIndex("by_category_value", (q) => q.eq("category", args.category))
      .paginate(args.paginationOpts);
  },
});

export const countByCategory = query({
  args: {
    category: v.string(),
  },
  returns: v.number(),
  handler: async (ctx, args) => {
    const items = await ctx.db
      .query("paginatedItems")
      .withIndex("by_category_value", (q) => q.eq("category", args.category))
      .collect();
    return items.length;
  },
});

export const allByCategory = query({
  args: {
    category: v.string(),
  },
  returns: v.array(
    v.object({
      _id: v.id("paginatedItems"),
      _creationTime: v.number(),
      value: v.number(),
      category: v.string(),
    }),
  ),
  handler: async (ctx, args) => {
    return await ctx.db
      .query("paginatedItems")
      .withIndex("by_category_value", (q) => q.eq("category", args.category))
      .collect();
  },
});
