import { v } from "convex/values";
import { mutation, query, internalMutation } from "./_generated/server";
import { internal } from "./_generated/api";

// Scheduled jobs scenario: mutations schedule future mutations that
// increment a shared counter. Each scheduled job must execute exactly once.
//
// Flow: schedule() creates a job that will run _executeJob, which
// increments the counter. We verify the final counter value equals
// the number of jobs scheduled.

export const setup = mutation({
  args: {},
  returns: v.id("scheduledCounter"),
  handler: async (ctx) => {
    return await ctx.db.insert("scheduledCounter", {
      count: 0,
      jobsScheduled: 0,
    });
  },
});

export const scheduleJob = mutation({
  args: {
    counterId: v.id("scheduledCounter"),
    delayMs: v.number(),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    // Increment jobsScheduled counter atomically with scheduling.
    const counter = await ctx.db.get(args.counterId);
    if (!counter) throw new Error("Counter not found");
    await ctx.db.patch(args.counterId, {
      jobsScheduled: counter.jobsScheduled + 1,
    });

    // Schedule the job to run after a delay.
    await ctx.scheduler.runAfter(args.delayMs, internal.scheduled._executeJob, {
      counterId: args.counterId,
    });
    return null;
  },
});

export const _executeJob = internalMutation({
  args: {
    counterId: v.id("scheduledCounter"),
  },
  handler: async (ctx, args) => {
    const counter = await ctx.db.get(args.counterId);
    if (!counter) throw new Error("Counter not found in scheduled job");
    await ctx.db.patch(args.counterId, { count: counter.count + 1 });
  },
});

export const getCounter = query({
  args: {
    counterId: v.id("scheduledCounter"),
  },
  returns: v.object({
    count: v.number(),
    jobsScheduled: v.number(),
  }),
  handler: async (ctx, args) => {
    const counter = await ctx.db.get(args.counterId);
    if (!counter) throw new Error("Counter not found");
    return { count: counter.count, jobsScheduled: counter.jobsScheduled };
  },
});
