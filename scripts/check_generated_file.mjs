import { readFile } from "node:fs/promises";

const [expectedPath, actualPath] = process.argv.slice(2);
if (!expectedPath || !actualPath) {
  throw new Error("usage: check_generated_file <expected> <actual>");
}
const [expected, actual] = await Promise.all([
  readFile(expectedPath),
  readFile(actualPath),
]);
if (!expected.equals(actual)) {
  throw new Error(`${expectedPath} is stale; regenerate it from the Rust palette catalog.`);
}
