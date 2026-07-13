import type { APIRequestContext } from "@playwright/test";
import { extract } from "letterparser";

export interface CapturedMessage {
  id: string;
  from: string;
  to: string[];
  subject: string;
  raw: string;        // full RFC822 MIME
  attachments: CapturedAttachment[];
}

export interface CapturedAttachment {
  filename: string;
  contentType: string;
  content: Buffer;    // decoded bytes
}

const MAILHOG_API = process.env.MAILHOG_API ?? "http://localhost:8025";

export async function clearMailhog(request: APIRequestContext): Promise<void> {
  await request.delete(`${MAILHOG_API}/api/v2/messages`);
}

export async function waitForMessage(
  request: APIRequestContext,
  predicate: (m: CapturedMessage) => boolean,
  opts: { timeoutMs?: number; intervalMs?: number } = {},
): Promise<CapturedMessage> {
  const timeoutMs = opts.timeoutMs ?? 15_000;
  const intervalMs = opts.intervalMs ?? 500;
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    const resp = await request.get(`${MAILHOG_API}/api/v2/messages`);
    const body = await resp.json();
    for (const item of body.items ?? []) {
      const msg = parseMailhogItem(item);
      if (predicate(msg)) return msg;
    }
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  throw new Error(`No MailHog message matched predicate within ${timeoutMs}ms`);
}

function parseMailhogItem(item: any): CapturedMessage {
  const headers = item.Content?.Headers ?? {};
  const raw = item.Raw?.Data ?? "";
  return {
    id: item.ID,
    from: headers.From?.[0] ?? "",
    to: (headers.To ?? []).flatMap((t: string) => t.split(",").map((s) => s.trim())),
    subject: headers.Subject?.[0] ?? "",
    raw,
    attachments: parseMimeAttachments(raw),
  };
}

function parseMimeAttachments(raw: string): CapturedAttachment[] {
  const mail = extract(raw);
  const attachments: CapturedAttachment[] = [];
  if (mail.attachments) {
    for (const att of mail.attachments) {
      const filename = att.filename ?? "unnamed";
      const contentType = att.contentType?.type ?? "application/octet-stream";
      let content: Buffer;
      if (att.body instanceof Uint8Array) {
        content = Buffer.from(att.body);
      } else if (typeof att.body === "string") {
        content = Buffer.from(att.body, "utf-8");
      } else {
        content = Buffer.alloc(0);
      }
      attachments.push({ filename, contentType, content });
    }
  }
  return attachments;
}
