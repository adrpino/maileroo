import { test, expect } from "../fixtures/app";
import { waitForMessage } from "../helpers/mailhog";

const MAILHOG_API = process.env.MAILHOG_API ?? "http://localhost:8025";

// Creates a fresh API key through the dashboard and returns the token.
async function createApiKey(page: import("@playwright/test").Page, name: string): Promise<string> {
  await page.goto("/api-keys");
  await page.fill('input[name="name"]', name);
  await page.click('form[hx-post="/api-keys"] button[type="submit"]');
  const keyText = await page
    .locator("code.key-display")
    .textContent({ timeout: 10_000 });
  expect(keyText).toBeTruthy();
  return keyText!.trim();
}

test("JSON API send delivers through SMTP to MailHog", async ({
  page,
  request,
  cleanMailhog,
  seededAlias,
}) => {
  const apiKey = await createApiKey(page, "e2e-send-key");

  // 1. The aliases endpoint lists the seeded alias with its full address.
  const aliasesResp = await request.get("/api/v1/aliases", {
    headers: { Authorization: `Bearer ${apiKey}` },
  });
  expect(aliasesResp.status()).toBe(200);
  const aliasesBody = await aliasesResp.json();
  const addresses = aliasesBody.aliases.map((a: { address: string }) => a.address);
  expect(addresses).toContain(seededAlias);

  // 2. Send via the JSON API with alias resolution by address.
  const sendResp = await request.post("/api/v1/emails/send", {
    headers: { Authorization: `Bearer ${apiKey}` },
    data: {
      from_alias: seededAlias,
      to: "api-recipient@e2e.test",
      subject: "E2E API send",
      body: "Sent through the JSON API.",
    },
  });
  expect(sendResp.status()).toBe(200);
  const sentBody = await sendResp.json();
  expect(sentBody.status).toBe("sent");
  expect(sentBody.message_id).toBeTruthy();
  expect(sentBody.email_id).toBeTruthy();

  // 3. MailHog captured exactly one message with the right envelope.
  const msg = await waitForMessage(request, (m) => m.subject === "E2E API send");
  expect(msg.from).toContain(seededAlias);
  expect(msg.to).toContain("api-recipient@e2e.test");

  void cleanMailhog;
});

test("JSON API dry run does not deliver", async ({
  page,
  request,
  cleanMailhog,
  seededAlias,
}) => {
  const apiKey = await createApiKey(page, "e2e-dry-run-key");

  const resp = await request.post("/api/v1/emails/send", {
    headers: { Authorization: `Bearer ${apiKey}` },
    data: {
      from_alias: seededAlias,
      to: "dry-run@e2e.test",
      subject: "E2E API dry run",
      body: "Should never leave the building.",
      dry_run: true,
    },
  });
  expect(resp.status()).toBe(200);
  const body = await resp.json();
  expect(body.status).toBe("dry_run");
  expect(body.resolved_from).toBe(seededAlias);
  expect(body.message_id).toBeTruthy();

  // Nothing with this subject ever reached MailHog (count-based assertions
  // race with other parallel specs, so we filter instead).
  await page.waitForTimeout(500);
  const all = await request.get(`${MAILHOG_API}/api/v2/messages`).then((r) => r.json());
  const subjects: string[] = (all.messages ?? []).map((m: { Subject: string }) => m.Subject);
  expect(subjects).not.toContain("E2E API dry run");

  void cleanMailhog;
});
