import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRoute,
} from "@tanstack/react-router";
import type { ReactNode } from "react";
import "../index.css";

export const Route = createRootRoute({
  notFoundComponent: NotFound,
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1.0" },
      {
        name: "description",
        content:
          "Captures — a work-in-progress, cross-platform screen capture utility by Jose Valerio.",
      },
      { name: "theme-color", content: "#101014" },
      { title: "Captures" },
    ],
    links: [
      { rel: "icon", type: "image/png", href: "/favicon.png" },
      { rel: "preconnect", href: "https://fonts.googleapis.com" },
      {
        rel: "preconnect",
        href: "https://fonts.gstatic.com",
        crossOrigin: "anonymous",
      },
      {
        rel: "stylesheet",
        href: "https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600;700&display=swap",
      },
    ],
  }),
  component: RootComponent,
});

function RootComponent() {
  return (
    <RootDocument>
      <Outlet />
    </RootDocument>
  );
}

function NotFound() {
  return (
    <main className="mx-auto flex min-h-dvh w-full max-w-xl flex-col justify-center px-6">
      <p className="text-sm text-ink-muted">404</p>
      <h1 className="mt-2 text-2xl font-medium tracking-tight text-ink">Page not found.</h1>
      <p className="mt-3 text-sm leading-relaxed text-ink-muted">
        The requested page does not exist.
      </p>
      <a href="/" className="mt-6 font-semibold text-ink underline-offset-4 hover:underline">
        Return to Captures
      </a>
    </main>
  );
}

function RootDocument({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en" style={{ colorScheme: "dark", backgroundColor: "#101014" }}>
      <head>
        <HeadContent />
      </head>
      <body style={{ backgroundColor: "#101014", color: "#f2f2f4" }}>
        {children}
        <Scripts />
      </body>
    </html>
  );
}
