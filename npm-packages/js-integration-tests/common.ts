import fs from "fs";
import path from "path";

// Local backend (convex-local-backend) defaults to port 3210; backend-harness
// uses 8000/8001. Set DEPLOYMENT_URL when using a local backend.
export const deploymentUrl =
  process.env.DEPLOYMENT_URL || "http://127.0.0.1:8000";
export const siteUrl = process.env.SITE_URL || "http://127.0.0.1:8001";

// Resolve admin key path from this package (js-integration-tests) to repo crates/
const defaultAdminKeyPath = path.join(
  __dirname,
  "..",
  "..",
  "..",
  "crates",
  "keybroker",
  "dev",
  "admin_key.txt",
);
const adminKeyPath =
  process.env.ADMIN_KEY_PATH ||
  (fs.existsSync(defaultAdminKeyPath) ? defaultAdminKeyPath : path.join(process.cwd(), "..", "..", "crates", "keybroker", "dev", "admin_key.txt"));

export const adminKey =
  process.env.ADMIN_KEY ||
  fs.readFileSync(adminKeyPath, "utf8").trim();
