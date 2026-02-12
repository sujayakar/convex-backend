import { v } from "convex/values";
import { mutation, query } from "./_generated/server";

// Table: events with user_id and timestamp, indexed by [user_id, timestamp].
// Scenario: concurrent inserts of events for different users, interleaved
// with index range queries that scan a specific user's events in a time range.
// Invariant: query results always reflect a consistent snapshot and contain
// exactly the matching documents in correct order.

export const insertEvent = mutation({
  args: {
    userId: v.number(),
    timestamp: v.number(),
    payload: v.string(),
  },
  returns: v.id("events"),
  handler: async (ctx, args) => {
    return await ctx.db.insert("events", {
      userId: args.userId,
      timestamp: args.timestamp,
      payload: args.payload,
    });
  },
});

export const queryUserEvents = query({
  args: {
    userId: v.number(),
  },
  returns: v.array(
    v.object({
      _id: v.id("events"),
      _creationTime: v.number(),
      userId: v.number(),
      timestamp: v.number(),
      payload: v.string(),
    }),
  ),
  handler: async (ctx, args) => {
    return await ctx.db
      .query("events")
      .withIndex("by_user_timestamp", (q) => q.eq("userId", args.userId))
      .collect();
  },
});

export const queryUserEventsInRange = query({
  args: {
    userId: v.number(),
    minTimestamp: v.number(),
    maxTimestamp: v.number(),
  },
  returns: v.array(
    v.object({
      _id: v.id("events"),
      _creationTime: v.number(),
      userId: v.number(),
      timestamp: v.number(),
      payload: v.string(),
    }),
  ),
  handler: async (ctx, args) => {
    return await ctx.db
      .query("events")
      .withIndex("by_user_timestamp", (q) =>
        q
          .eq("userId", args.userId)
          .gte("timestamp", args.minTimestamp)
          .lte("timestamp", args.maxTimestamp),
      )
      .collect();
  },
});

// Count all events per user -- used for validation.
export const countByUser = query({
  args: {},
  returns: v.any(),
  handler: async (ctx) => {
    const counts: Record<number, number> = {};
    const allEvents = await ctx.db.query("events").collect();
    for (const event of allEvents) {
      counts[event.userId] = (counts[event.userId] || 0) + 1;
    }
    return counts;
  },
});
