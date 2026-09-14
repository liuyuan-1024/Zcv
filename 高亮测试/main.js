import { readFile } from "node:fs/promises";

const retries = 3;
const config = { enabled: true, name: "Zcv" };

function greeting(name) {
  return `Hello, ${name}!`;
}

const run = async () => {
  if (!config.enabled) return null;
  const text = await readFile("README.md", "utf8");
  console.log(greeting(config.name), text.length, retries);
};

run().catch(console.error);
