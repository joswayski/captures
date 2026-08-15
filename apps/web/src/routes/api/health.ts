import { createFileRoute } from "@tanstack/react-router";
import { env } from "cloudflare:workers";
import { handleRequest, type ApiEnv } from "../../server/api";

export const Route = createFileRoute("/api/health")({
  server: {
    handlers: {
      GET: ({ request }) =>
        handleRequest(request, env as unknown as ApiEnv),
    },
  },
});
