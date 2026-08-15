import tanstackHandler from "@tanstack/react-start/server-entry";

import { handleApiRequest, type WorkerEnv } from "./api";

function isApiRequest(request: Request): boolean {
  const pathname = new URL(request.url).pathname;
  return pathname === "/api" || pathname.startsWith("/api/");
}

export default {
  async fetch(request: Request, env: WorkerEnv): Promise<Response> {
    if (isApiRequest(request)) {
      return handleApiRequest(request, env);
    }

    return tanstackHandler.fetch(request);
  },
  // Add queue(), scheduled(), and other Cloudflare event handlers here when configured.
} satisfies ExportedHandler<WorkerEnv>;
