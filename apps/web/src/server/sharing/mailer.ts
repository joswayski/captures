import nodemailer from "nodemailer";
import type { Mailer, SharingConfig } from "./types.ts";

export function createSesSmtpMailer(config: SharingConfig["mail"]): Mailer {
  if (!config.host || !config.user || !config.password) {
    throw new Error("SES SMTP is not configured");
  }
  const transport = nodemailer.createTransport({
    host: config.host,
    port: config.port,
    secure: config.secure,
    auth: { user: config.user, pass: config.password },
    requireTLS: !config.secure,
    connectionTimeout: 10_000,
    greetingTimeout: 10_000,
    socketTimeout: 15_000,
  });

  return {
    async sendLoginCode(email, code) {
      const result = await transport.sendMail({
        from: config.from,
        to: email,
        subject: `${code} is your Captures sign-in code`,
        text: [
          `Your Captures sign-in code is ${code}.`,
          "",
          "It expires in 10 minutes. If you did not request it, you can ignore this email.",
        ].join("\n"),
        html: `<p>Your Captures sign-in code is:</p><p style="font-size:28px;font-weight:700;letter-spacing:0.18em">${code}</p><p>It expires in 10 minutes. If you did not request it, you can ignore this email.</p>`,
        headers: {
          "X-SES-CONFIGURATION-SET": config.configurationSet,
          "X-SES-TENANT": config.tenant,
        },
      });
      return result.messageId;
    },
  };
}
