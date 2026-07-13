import { test, expect } from "../fixtures/app";

test("autosave persists text but not files", async ({ page, request, cleanMailhog }) => {
  await page.goto("/dashboard");
  await page.click('button[hx-get="/api/v1/emails/compose"]');
  await page.fill('input[name="to_email"]', "draft@e2e.test");
  await page.fill('input[name="subject"]', "Draft autosave");
  await page.fill('textarea[name="body_text"]', "Typing a draft...");
  await page.setInputFiles('input[name="attachments"]', {
    name: "should-not-upload.txt", mimeType: "text/plain",
    buffer: Buffer.from("must not reach /drafts"),
  });

  // Trigger autosave manually (the form fires on input change + 2s delay)
  await page.fill('textarea[name="body_text"]', "Typing more...");
  await page.waitForResponse((r) => r.url().includes("/api/v1/emails/drafts") && r.ok(), { timeout: 10_000 });

  // The draft row exists. Go to drafts folder.
  await page.goto("/dashboard?folder=drafts");
  await page.click('tr:has-text("Draft autosave") .btn-view');
  
  // Confirm the file input is empty
  await expect(page.locator('input[name="attachments"]')).toHaveValue("");
});
