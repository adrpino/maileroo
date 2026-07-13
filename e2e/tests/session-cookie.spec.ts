import { test, expect } from "../fixtures/app";
import fs from "fs";
import path from "path";

test.describe("Session and CSRF Cookie Expiration", () => {
  // Use the global logged-in state just to ensure we start clean
  test.use({ storageState: "e2e/.auth/admin.json" });

  test("should persist CSRF token after browser restart due to Max-Age matching server session", async ({ page, browser }) => {
    const baseURL = process.env.E2E_BASE_URL ?? "http://localhost:3001";
    
    // 1. Visit dashboard to confirm we are logged in
    await page.goto(`${baseURL}/dashboard`);
    await expect(page.locator("h1")).toContainText("Dashboard");
    
    // Send a test email so we have something to delete
    await page.click('button[hx-get="/api/v1/emails/compose"]');
    await page.fill('input[name="to_email"]', "test-delete@example.com");
    await page.fill('input[name="subject"]', "Test email to delete");
    await page.fill('textarea[name="body_text"]', "This should be deleted after browser restart.");
    await page.click('button.btn-send');
    await expect(page.locator('text=Email sent successfully!')).toBeVisible({ timeout: 10_000 });
    
    // Wait for the modal to close and reload dashboard so the sent email appears
    await expect(page.locator('#maileroo-modal')).toHaveCount(0);
    await page.reload();

    // 2. Validate current state has the csrf_token
    const context = page.context();
    let cookies = await context.cookies();
    const originalCsrfCookie = cookies.find(c => c.name === "csrf_token");
    
    expect(originalCsrfCookie).toBeDefined();
    // This assertion fails right now because expires is -1 (session cookie)
    expect(originalCsrfCookie!.expires).toBeGreaterThan(0);

    // 3. Simulate Browser Restart
    // Playwright's storageState automatically strips session cookies (expires: -1).
    // By saving state, closing the context, and loading it in a new context, 
    // we perfectly simulate a user closing and opening their browser application.
    const statePath = path.join(__dirname, "../.auth/temp_restart_state.json");
    await context.storageState({ path: statePath });
    
    // Open a completely fresh browser context loaded with the saved state
    const newContext = await browser.newContext({ storageState: statePath });
    const newPage = await newContext.newPage();
    
    // 4. Return to the dashboard. The `tower_sessions` cookie survived, so we are still logged in.
    await newPage.goto(`${baseURL}/dashboard`);
    await expect(newPage.locator("h1")).toContainText("Dashboard");

    // 5. Attempt a state-changing action (DELETE).
    // If the CSRF cookie was dropped (because it was a session cookie), HTMX won't send it, 
    // and the server will return 403, failing to execute the action.
    
    // Check one of the existing seeded emails and click the batch delete button
    const emailRow = newPage.locator('tr.unread').first();
    await emailRow.locator('.email-checkbox').check();
    await newPage.locator('#btn-batch-delete').click();

    // Confirm the deletion modal
    const deleteModal = newPage.locator('#maileroo-modal');
    await expect(deleteModal).toBeVisible();
    await deleteModal.locator('button.maileroo-btn-danger').click();
    
    // Wait for the modal to close
    await expect(deleteModal).toBeHidden();
    
    // Cleanup temp state
    if (fs.existsSync(statePath)) {
      fs.unlinkSync(statePath);
    }
  });
});