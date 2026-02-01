/**
 * Backend server configuration constants.
 *
 * These values can be overridden via environment variables for flexibility
 * in different deployment environments.
 */

/**
 * Default character threshold for chunking content during streaming.
 * This controls how many characters are accumulated before processing a chunk
 * (e.g., for thinking summaries).
 *
 * Can be overridden via the CHUNK_CHAR_THRESHOLD environment variable.
 *
 * @default 400
 */
export const DEFAULT_CHUNK_CHAR_THRESHOLD =
  parseInt(process.env.CHUNK_CHAR_THRESHOLD ?? "", 10) || 400;
