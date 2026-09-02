import { createFileRoute } from "@tanstack/react-router";

import { createFeedback, preflightFeedback } from "../../server/api.ts";
import { getApiEnv } from "../../server/env.ts";

export const Route = createFileRoute("/api/feedback")({
  server: {
    handlers: {
      OPTIONS: ({ request }) => preflightFeedback(request),
      POST: ({ request }) => createFeedback(request, getApiEnv()),
    },
  },
});
