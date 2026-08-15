import { createFileRoute } from "@tanstack/react-router";
import Home from "../pages/Home";

export const Route = createFileRoute("/")({
  loader: () => ({
    initialNow: Date.now(),
    latestChanges: __LATEST_CHANGES__,
  }),
  staleTime: Number.POSITIVE_INFINITY,
  component: HomeRoute,
});

function HomeRoute() {
  return <Home {...Route.useLoaderData()} />;
}
