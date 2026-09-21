import {
  createCsrfMiddleware,
  createMiddleware,
  createStart,
} from "@tanstack/react-start";

const accountResponses = createMiddleware().server(
  async ({ request, next }) => {
    const result = await next();
    const pathname = new URL(request.url).pathname;
    if (pathname === "/dashboard" || pathname.startsWith("/dashboard/") || pathname.startsWith("/s/")) {
      result.response.headers.set("Cache-Control", "no-store");
      result.response.headers.set("Referrer-Policy", "no-referrer");
    }
    if (pathname === "/dashboard" || pathname.startsWith("/dashboard/")) {
      result.response.headers.set("X-Robots-Tag", "noindex, nofollow");
    }
    return result;
  },
);

export const startInstance = createStart(() => ({
  requestMiddleware: [
    accountResponses,
    // A custom start instance replaces Start's default CSRF middleware.
    createCsrfMiddleware({ filter: (ctx) => ctx.handlerType === "serverFn" }),
  ],
}));
