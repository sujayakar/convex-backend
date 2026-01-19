import { mutation, query } from "./_generated/server";
import { v } from "convex/values";
import { PaginationOptions } from "convex/server";

export const insertUser = mutation({
  args: {
    name: v.string(),
    email: v.optional(v.string()),
    age: v.optional(v.number()),
    bio: v.optional(v.string()),
  },
  handler: async ({ db }, args) => {
    return await db.insert("users", args);
  },
});

export const selectNameOnly = query({
  args: {},
  handler: async ({ db }) => {
    return await db.query("users").select(["name"]).collect();
  },
});

export const selectMultipleFields = query({
  args: {},
  handler: async ({ db }) => {
    return await db.query("users").select(["name", "email"]).collect();
  },
});

export const selectWithFilter = query({
  args: { minAge: v.number() },
  handler: async ({ db }, args) => {
    return await db
      .query("users")
      .filter((q) => q.gte(q.field("age"), args.minAge))
      .select(["name", "age"])
      .collect();
  },
});

export const selectFirst = query({
  args: {},
  handler: async ({ db }) => {
    return await db.query("users").select(["name"]).first();
  },
});

export const selectEmpty = query({
  args: {},
  handler: async ({ db }) => {
    return await db.query("users").select([]).collect();
  },
});

export const selectWithIndex = query({
  args: {},
  handler: async ({ db }) => {
    return await db
      .query("users")
      .withIndex("by_creation_time")
      .select(["name"])
      .take(5);
  },
});

export const selectWithPagination = query({
  args: { paginationOpts: v.any() },
  handler: async (
    { db },
    { paginationOpts }: { paginationOpts: PaginationOptions },
  ) => {
    return await db.query("users").select(["name"]).paginate(paginationOpts);
  },
});

export const deleteAllUsers = mutation({
  args: {},
  handler: async ({ db }) => {
    const users = await db.query("users").collect();
    for (const user of users) {
      await db.delete(user._id);
    }
  },
});
