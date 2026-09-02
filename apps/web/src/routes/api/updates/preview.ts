import { createFileRoute } from "@tanstack/react-router";

import { getPreviewUpdaterManifest } from "../../../server/api.ts";

export const Route = createFileRoute("/api/updates/preview")({
  server: {
    handlers: {
      GET: ({ request }) => getPreviewUpdaterManifest(request),
    },
  },
});
