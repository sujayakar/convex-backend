import { v } from "convex/values";
import { mutation, query } from "./_generated/server";

// Subscription scenario: test that subscriptions reflect mutations.
// A query watches a counter; concurrent mutations increment it.
// The subscription should eventually converge to the correct value.

export const initCounter = mutation({
  args: {},
  returns: v.id("subCounter"),
  handler: async (ctx) => {
    return await ctx.db.insert("subCounter", { value: 0 });
  },
});

export const increment = mutation({
  args: {
    counterId: v.id("subCounter"),
  },
  returns: v.number(),
  handler: async (ctx, args) => {
    const doc = await ctx.db.get(args.counterId);
    if (!doc) throw new Error("Counter not found");
    const newValue = doc.value + 1;
    await ctx.db.replace(args.counterId, { value: newValue });
    return newValue;
  },
});

export const getCounter = query({
  args: {
    counterId: v.id("subCounter"),
  },
  returns: v.number(),
  handler: async (ctx, args) => {
    const doc = await ctx.db.get(args.counterId);
    if (!doc) throw new Error("Counter not found");
    return doc.value;
  },
});
