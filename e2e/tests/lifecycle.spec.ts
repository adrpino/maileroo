import { test, expect } from "../fixtures/app";
import { waitForMessage } from "../helpers/mailhog";
import { deliverInbound } from "../helpers/smtp";

test("full lifecycle: inbound → reply → outbound → dashboard", async ({ page, request, cleanMailhog, seededAlias }) => {
  // 1. Inbound email arrives.
  const rawMime = [
    "From: sender@external.test",
    `To: ${seededAlias}`,
    "Subject: Lifecycle test",
    "",
    "Original inbound message.",
  ].join("\r\n");
  
  await deliverInbound("127.0.0.1:2526", "sender@external.test", seededAlias, rawMime);

  // 2. Dashboard shows it
  await page.goto("/dashboard");
  await expect(page.locator('text=Lifecycle test')).toBeVisible({ timeout: 10_000 });

  // 3. Open detail, reply.
  await page.click('tr:has-text("Lifecycle test") .btn-view');
  await page.fill('textarea[name="body_text"]', "Outbound reply.");
  await page.click('button.btn-reply-send');

  // 4. Reply is captured by MailHog.
  const reply = await waitForMessage(request, (m) => m.raw.includes("Outbound reply."));
  expect(reply.subject).toContain("Lifecycle test");
  expect(reply.to).toContain("sender@external.test");
});
