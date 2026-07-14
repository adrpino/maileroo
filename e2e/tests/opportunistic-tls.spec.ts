import { test, expect } from "../fixtures/app";
import { waitForMessage } from "../helpers/mailhog";

test("opportunistic TLS end to end delivery", async ({ page, request, cleanMailhog, seededAlias }) => {
  await page.goto("/dashboard");
  // Click the Compose button via hx-get attribute to be robust across locales
  await page.click('button[hx-get="/api/v1/emails/compose"]');

  // Fill in the form
  await page.fill('input[name="to_email"]', "recipient@e2e.test");
  await page.fill('input[name="subject"]', "Opportunistic TLS test");
  await page.fill('textarea[name="body_text"]', "Testing opportunistic STARTTLS.");
  await page.click('button.btn-send');

  // 1. Toast appears
  await expect(page.locator('text=Email sent successfully!')).toBeVisible({ timeout: 10_000 });

  // 2. MailHog captured the message
  const msg = await waitForMessage(request, (m) => m.subject === "Opportunistic TLS test");
  expect(msg.from).toContain(seededAlias);
  expect(msg.raw).toContain("Testing opportunistic STARTTLS.");

  // Modal auto-closes on success
  await expect(page.locator('#maileroo-modal')).toHaveCount(0);
});
