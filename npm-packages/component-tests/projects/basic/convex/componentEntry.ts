import { query, mutation, action } from "./_generated/server";
import { components } from "./_generated/api";

export const hello = action({
  args: {},
  handler: async (ctx) => {
    return await ctx.runAction(components.envVars.messages.hello, {});
  },
});

export const envVarQuery = query({
  args: {},
  handler: async (ctx) => {
    return await ctx.runQuery(components.envVars.messages.envVarQuery, {});
  },
});
export const envVarAction = action({
  args: {},
  handler: async (ctx) => {
    return await ctx.runAction(components.envVars.messages.envVarAction, {});
  },
});
export const systemEnvVarQuery = query({
  args: {},
  handler: async (ctx) => {
    return await ctx.runQuery(
      components.envVars.messages.systemEnvVarQuery,
      {},
    );
  },
});
export const systemEnvVarAction = action({
  args: {},
  handler: async (ctx) => {
    return await ctx.runAction(
      components.envVars.messages.systemEnvVarAction,
      {},
    );
  },
});

export const dateNow = query({
  args: {},
  handler: async (ctx) => {
    const myDateNow = Date.now();
    const componentDateNow = await ctx.runQuery(
      components.component.messages.dateNow,
      {},
    );
    return [myDateNow, componentDateNow];
  },
});

export const mathRandom = query({
  args: {},
  handler: async (ctx) => {
    const componentRandom = await ctx.runQuery(
      components.component.messages.mathRandom,
      {},
    );
    const myRandom = Math.random();
    return [myRandom, componentRandom];
  },
});

// Test: two concurrent query subtransactions via Promise.all
export const concurrentQueries = mutation({
  args: {},
  handler: async (ctx) => {
    const [messages, dateNow] = await Promise.all([
      ctx.runQuery(components.component.messages.listMessages, {}),
      ctx.runQuery(components.component.messages.dateNow, {}),
    ]);
    return { messages, dateNow };
  },
});
