import { safeEqual } from "./crypto.ts";

export interface LoginCodeAttempt {
  id: string;
  codeHmac: Buffer;
  attempts: number;
  expiresAt: Date;
}

export async function verifyLoginCodeAttempt<TClient, TResult>(
  client: TClient,
  options: {
    findLoginCode: (client: TClient) => Promise<LoginCodeAttempt | null>;
    expectedCodeHmac: Buffer;
    recordFailedAttempt: (client: TClient, id: string) => Promise<void>;
    consumeLoginCode: (client: TClient, id: string) => Promise<void>;
    onSuccess: (client: TClient) => Promise<TResult>;
    now?: number;
  },
): Promise<TResult | null> {
  const loginCode = await options.findLoginCode(client);
  if (
    !loginCode ||
    loginCode.expiresAt.getTime() <= (options.now ?? Date.now()) ||
    loginCode.attempts >= 5 ||
    !safeEqual(loginCode.codeHmac, options.expectedCodeHmac)
  ) {
    if (loginCode) await options.recordFailedAttempt(client, loginCode.id);
    // Returning commits the failed-attempt update. Throwing here would roll it
    // back and defeat the five-attempt limit.
    return null;
  }
  await options.consumeLoginCode(client, loginCode.id);
  return options.onSuccess(client);
}
