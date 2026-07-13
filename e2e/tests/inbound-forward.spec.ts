import { test, expect } from "../fixtures/app";
import { waitForMessage } from "../helpers/mailhog";
import { deliverInbound } from "../helpers/smtp";

test("inbound mail to an auto-forwarding alias relays to MailHog", async ({ page, request, cleanMailhog, seededAlias }) => {
  // 1. Deliver a raw RFC822 message into the app's inbound SMTP port (127.0.0.1:2526).
  const rawMime = [
    "From: outside@external.test",
    `To: ${seededAlias}`,
    "Subject: Inbound forward test",
    "",
    "This should be auto-forwarded to MailHog.",
  ].join("\r\n");
  
  await deliverInbound("127.0.0.1:2526", "outside@external.test", seededAlias, rawMime);

  // 2. The forwarded copy lands in MailHog
  const forwarded = await waitForMessage(request, (m) => m.subject === "Inbound forward test");
  expect(forwarded.raw).toContain("This should be auto-forwarded to MailHog.");

  // 3. Dashboard shows the new email
  await page.goto("/dashboard");
  await expect(page.locator('text=Inbound forward test')).toBeVisible({ timeout: 10_000 });
});
