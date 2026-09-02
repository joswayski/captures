import { createFileRoute } from "@tanstack/react-router";

import { getHealth } from "../../server/api.ts";

export const Route = createFileRoute("/api/health")({
  server: {
    handlers: {
      GET: ({ request }) => getHealth(request),
    },
  },
});
