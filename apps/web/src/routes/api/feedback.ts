import { createFileRoute } from "@tanstack/react-router";
import { env } from "cloudflare:workers";
import { handleRequest, type ApiEnv } from "../../server/api";

function handleApiRequest(request: Request) {
  return handleRequest(request, env as unknown as ApiEnv);
}

export const Route = createFileRoute("/api/feedback")({
  server: {
    handlers: {
      POST: ({ request }) => handleApiRequest(request),
      OPTIONS: ({ request }) => handleApiRequest(request),
    },
  },
});
