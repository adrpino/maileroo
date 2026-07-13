import { test, expect } from "../fixtures/app";

test("DKIM rotation generates a new key and DNS verification fails cleanly without records", async ({ page, seededDomain }) => {
  await page.goto("/domains");
  
  // Click the "DKIM Settings" button for our seeded domain
  await page.click(`tr:has-text("${seededDomain}") button:has-text("DKIM Settings")`);
  
  // Wait for the modal to open
  await expect(page.locator("id=dkim-modal-wrapper")).toBeVisible({ timeout: 10_000 });
  
  // Handle the confirm dialog that HTMX triggers for hx-confirm
  page.once('dialog', async dialog => {
    await dialog.accept();
  });

  // Click the generate / rotate button to create a pending key
  await page.click('button[hx-post*="/rotate-dkim"]');
  
  // Verify that the pending DKIM key section appears
  await expect(page.locator('button[hx-post*="/verify-dkim"]')).toBeVisible({ timeout: 15_000 });
  
  // The modal shows the new selector + TXT value to publish
  const textContent = await page.locator("id=dkim-modal-wrapper").textContent();
  expect(textContent).toContain("v=DKIM1");

  // Click verify without publishing DNS — should fail/display verification failure gracefully
  await page.click('button[hx-post*="/verify-dkim"]');
  await expect(page.locator('id=dkim-modal-wrapper')).toContainText('DNS Verification failed', { timeout: 15_000 });
});
