import { v } from "convex/values";
import { mutation, query } from "./_generated/server";
import { Id } from "./_generated/dataModel";

export const setup = mutation({
  args: {
    size: v.number(),
  },
  returns: v.array(v.id("ring")),
  handler: async (ctx, args) => {
    const ids: Id<"ring">[] = [];
    // Create all documents, each pointing to the next (wrapping around).
    for (let i = 0; i < args.size; i++) {
      const next = (i + 1) % args.size;
      const id = await ctx.db.insert("ring", { idx: i, next });
      ids.push(id);
    }
    return ids;
  },
});

export const rotate = mutation({
  args: {
    ids: v.array(v.id("ring")),
    startIdx: v.number(),
    rotateLength: v.number(),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    const { ids, startIdx, rotateLength } = args;

    // Walk rotateLength + 1 steps from startIdx, collecting indices.
    const chain: number[] = [startIdx];
    let current = startIdx;
    for (let i = 0; i < rotateLength + 1; i++) {
      const doc = await ctx.db.get(ids[current]);
      if (!doc) throw new Error(`Missing doc at index ${current}`);
      chain.push(doc.next);
      current = doc.next;
    }

    // chain = [a, b, c, d, e, f] with rotateLength=4:
    //   a->e, b->f, c->b, d->c (reverse the middle)
    const first = chain[0];
    const second = chain[1];
    const secondToLast = chain[chain.length - 2];
    const last = chain[chain.length - 1];

    await ctx.db.replace(ids[first], { idx: first, next: secondToLast });
    await ctx.db.replace(ids[second], { idx: second, next: last });

    let target = second;
    for (let i = 2; i < rotateLength + 1; i++) {
      const key = chain[i];
      await ctx.db.replace(ids[key], { idx: key, next: target });
      target = key;
    }
    return null;
  },
});

export const validate = query({
  args: {
    ids: v.array(v.id("ring")),
    expectedSize: v.number(),
  },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const { ids, expectedSize } = args;
    const visited = new Set<number>();
    visited.add(0);
    let current = 0;

    for (let hop = 0; hop < expectedSize - 1; hop++) {
      const doc = await ctx.db.get(ids[current]);
      if (!doc) throw new Error(`Missing doc at index ${current}`);
      const next = doc.next;
      if (next >= expectedSize) {
        throw new Error(`Next out of range: ${next}`);
      }
      if (visited.has(next)) {
        throw new Error(`Duplicate in ring after ${hop + 1} hops: ${next}`);
      }
      visited.add(next);
      current = next;
    }

    // After size-1 hops, should be back to 0.
    const lastDoc = await ctx.db.get(ids[current]);
    if (!lastDoc) throw new Error(`Missing doc at end: ${current}`);
    if (lastDoc.next !== 0) {
      throw new Error(`Ring didn't loop back: ${lastDoc.next}`);
    }
    return true;
  },
});
