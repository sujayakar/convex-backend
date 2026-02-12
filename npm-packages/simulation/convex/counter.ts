import { v } from "convex/values";
import { mutation, query } from "./_generated/server";

export const setup = mutation({
  args: {},
  returns: v.object({
    linearId: v.id("counterLinear"),
    triangleId: v.id("counterTriangle"),
  }),
  handler: async (ctx) => {
    const linearId = await ctx.db.insert("counterLinear", { count: 0 });
    const triangleId = await ctx.db.insert("counterTriangle", { count: 0 });
    return { linearId, triangleId };
  },
});

export const increment = mutation({
  args: {
    linearId: v.id("counterLinear"),
    triangleId: v.id("counterTriangle"),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    const linearDoc = await ctx.db.get(args.linearId);
    if (!linearDoc) throw new Error("Linear doc not found");
    const triangleDoc = await ctx.db.get(args.triangleId);
    if (!triangleDoc) throw new Error("Triangle doc not found");

    const newLinear = linearDoc.count + 1;
    await ctx.db.replace(args.linearId, { count: newLinear });

    // Verify within-transaction read consistency.
    const check = await ctx.db.get(args.linearId);
    if (!check || check.count !== newLinear) {
      throw new Error("Within-transaction read inconsistency!");
    }

    await ctx.db.replace(args.triangleId, {
      count: triangleDoc.count + newLinear,
    });
    return null;
  },
});

export const validate = query({
  args: {
    linearId: v.id("counterLinear"),
    triangleId: v.id("counterTriangle"),
  },
  returns: v.object({ linear: v.number(), triangle: v.number() }),
  handler: async (ctx, args) => {
    const linearDoc = await ctx.db.get(args.linearId);
    if (!linearDoc) throw new Error("Linear doc not found");
    const triangleDoc = await ctx.db.get(args.triangleId);
    if (!triangleDoc) throw new Error("Triangle doc not found");

    const linear = linearDoc.count;
    const triangle = triangleDoc.count;
    const expected = ((linear + 1) * linear) / 2;
    if (triangle !== expected) {
      throw new Error(
        `Invariant violated: linear=${linear}, triangle=${triangle}, expected=${expected}`,
      );
    }
    return { linear, triangle };
  },
});
