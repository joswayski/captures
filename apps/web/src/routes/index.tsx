import { createFileRoute } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import { getRequestHeader, setResponseHeader } from "@tanstack/react-start/server";
import { detectPreviewDownloadIdFromRequest } from "../detectPreviewDownload";
import Home from "../pages/Home";

const HOMEPAGE_HINT_HEADERS = "User-Agent, Sec-CH-UA-Platform, Sec-CH-UA-Mobile";

const getPreviewDownloadId = createServerFn({ method: "GET" }).handler(() => {
  setResponseHeader("Vary", HOMEPAGE_HINT_HEADERS);
  setResponseHeader("Accept-CH", "Sec-CH-UA-Platform, Sec-CH-UA-Mobile");
  setResponseHeader("Cache-Control", "private");
  return detectPreviewDownloadIdFromRequest({
    userAgent: getRequestHeader("user-agent") ?? "",
    secChUaPlatform: getRequestHeader("sec-ch-ua-platform"),
    secChUaMobile: getRequestHeader("sec-ch-ua-mobile"),
  });
});

export const Route = createFileRoute("/")({
  loader: async () => ({
    initialNow: Date.now(),
    latestChanges: __LATEST_CHANGES__,
    previewDownloadId: await getPreviewDownloadId(),
  }),
  staleTime: Number.POSITIVE_INFINITY,
  component: HomeRoute,
});

function HomeRoute() {
  return <Home {...Route.useLoaderData()} />;
}
