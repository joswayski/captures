import { defineHandler } from "nitro";

import { handleApiRequest } from "../../src/server/api.ts";
import { getApiEnv } from "../../src/server/env.ts";

export default defineHandler((event) => {
  return handleApiRequest(event.req, getApiEnv());
});
