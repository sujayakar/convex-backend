import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

export default defineSchema({
  objects: defineTable(v.any()),

  // Elle scenario tables
  elleRegisters: defineTable({}),
  elleRegisterValues: defineTable({
    registerId: v.id("elleRegisters"),
    offset: v.number(),
    value: v.number(),
  }).index("registerId", ["registerId", "offset"]),

  // Counter scenario tables
  counterLinear: defineTable({ count: v.number() }),
  counterTriangle: defineTable({ count: v.number() }),

  // LinkRing scenario tables
  ring: defineTable({ idx: v.number(), next: v.number() }),

  // ScheduledJobs scenario tables
  scheduledCounter: defineTable({
    count: v.number(),
    jobsScheduled: v.number(),
  }),

  // Subscription scenario tables
  subCounter: defineTable({ value: v.number() }),

  // TextSearch scenario tables
  searchDocs: defineTable({
    title: v.string(),
    body: v.string(),
    category: v.string(),
  }).searchIndex("by_body", {
    searchField: "body",
    filterFields: ["category"],
  }),

  // Pagination scenario tables
  paginatedItems: defineTable({
    value: v.number(),
    category: v.string(),
  }).index("by_category_value", ["category", "value"]),

  // IndexQuery scenario tables
  events: defineTable({
    userId: v.number(),
    timestamp: v.number(),
    payload: v.string(),
  }).index("by_user_timestamp", ["userId", "timestamp"]),

  users: defineTable({
    name: v.string(),
    email: v.optional(v.string()),
  }).index("by_name", ["name"]),

  messages: defineTable({
    conversationId: v.id("conversations"),
    author: v.id("users"),
    body: v.string(),
  }).index("by_conversation", ["conversationId"]),
  conversationMembers: defineTable({
    conversationId: v.id("conversations"),
    userId: v.id("users"),
    hasUnreadMessages: v.boolean(),
    latestMessageTime: v.number(),
  })
    .index("by_conversation", ["conversationId"])
    .index("by_user_conversation", ["userId", "conversationId"])
    .index("by_latest_message_time", [
      "userId",
      "hasUnreadMessages",
      "latestMessageTime",
    ]),
  conversations: defineTable({
    emoji: v.optional(v.string()),
  }),
});
