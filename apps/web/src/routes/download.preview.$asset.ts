import { createFileRoute } from "@tanstack/react-router";
import { nativeDownload } from "../server/nativeDownload";

export const Route = createFileRoute("/download/preview/$asset")({
  server: {
    handlers: {
      GET: ({ params }) => nativeDownload(params.asset),
    },
  },
});
