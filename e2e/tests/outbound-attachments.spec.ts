import { test, expect } from "../fixtures/app";
import { waitForMessage } from "../helpers/mailhog";

test("compose + send with two attachments survives the wire", async ({ page, request, cleanMailhog, seededAlias }) => {
  await page.goto("/dashboard");
  // Click the Compose button via hx-get attribute to be robust across locales
  await page.click('button[hx-get="/api/v1/emails/compose"]');

  // Real <input type=file> upload
  const file1 = { name: "file1.txt", mimeType: "text/plain", buffer: Buffer.from("Hello, this is file one content!") };
  const file2 = { name: "file2.bin", mimeType: "application/octet-stream", buffer: Buffer.from([0x00, 0x01, 0x02, 0x03, 0x04]) };
  await page.setInputFiles('input[name="attachments"]', [
    { name: file1.name, mimeType: file1.mimeType, buffer: file1.buffer },
    { name: file2.name, mimeType: file2.mimeType, buffer: file2.buffer }
  ]);

  await page.fill('input[name="to_email"]', "recipient@e2e.test");
  await page.fill('input[name="subject"]', "E2E attachments test");
  await page.fill('textarea[name="body_text"]', "Two files attached.");
  await page.click('button.btn-send');

  // 1. Toast appears (HTMX swap into body).
  await expect(page.locator('text=Email sent successfully!')).toBeVisible({ timeout: 10_000 });

  // 2. MailHog captured the message — assert subject, from, to, and attachment count.
  const msg = await waitForMessage(request, (m) => m.subject === "E2E attachments test");
  expect(msg.from).toContain(seededAlias);
  expect(msg.to).toContain("recipient@e2e.test");
  expect(msg.attachments.length).toBe(2);
  expect(msg.attachments[0].filename).toBe("file1.txt");
  expect(msg.attachments[0].content.toString()).toBe("Hello, this is file one content!");
  expect(msg.attachments[1].filename).toBe("file2.bin");
  expect(Array.from(msg.attachments[1].content)).toEqual([0, 1, 2, 3, 4]);

  // 3. Sent-mail detail view lists the attachments (paperclip + download links).
  await page.goto("/dashboard?folder=sent");
  await page.click('tr:has-text("E2E attachments test") .btn-view');
  await expect(page.locator('a[href*="/attachment/"]')).toHaveCount(2);
  await expect(page.locator('text=file1.txt')).toBeVisible();
  await expect(page.locator('text=file2.bin')).toBeVisible();

  // 4. Download endpoint returns the correct bytes for each attachment
  const href = await page.locator('a:has-text("file1.txt")').first().getAttribute('href');
  expect(href).not.toBeNull();
  const dl1Resp = await request.get(new URL(href!, page.url()).toString());
  expect(dl1Resp.ok()).toBe(true);
  expect(await dl1Resp.body()).toEqual(file1.buffer);
});
