import { test, expect } from "../fixtures/app";
import { deliverInbound } from "../helpers/smtp";

test("favorites and mark unread lifecycle", async ({ page, seededAlias }) => {
  const subject = `Favorite & Unread Test ${Date.now()}`;
  const rawMime = [
    "From: sender@external.test",
    `To: ${seededAlias}`,
    `Subject: ${subject}`,
    "",
    "Testing favorites and unread toggling.",
  ].join("\r\n");

  // 1. Deliver an inbound email
  await deliverInbound("127.0.0.1:2526", "sender@external.test", seededAlias, rawMime);

  // 2. Open dashboard: email row should exist and have unread class
  await page.goto("/dashboard");
  const row = page.locator(`tr:has-text("${subject}")`);
  await expect(row).toBeVisible({ timeout: 10_000 });
  await expect(row).toHaveClass(/unread/);

  // 3. Open the email detail: marks it read
  await row.locator(".btn-view").click();
  await expect(page.locator("h1")).toContainText(subject);

  // 4. Click "Mark as Unread" on detail view -> redirects to dashboard
  const markUnreadBtn = page.locator('button[hx-post*="/mark-unread"]');
  await expect(markUnreadBtn).toBeVisible();
  await markUnreadBtn.click();
  await page.waitForURL("**/dashboard");

  // 5. Verify the email is unread again on dashboard
  const unreadRow = page.locator(`tr:has-text("${subject}")`);
  await expect(unreadRow).toHaveClass(/unread/);

  // 6. Click the star icon to favorite the email
  const starBtn = unreadRow.locator("button.btn-star");
  await expect(starBtn).toContainText("☆");
  await starBtn.click();
  await expect(starBtn).toContainText("★");

  // 7. Navigate to Favorites tab
  await page.click('a[href="/dashboard?folder=favorites"]');
  await page.waitForURL("**/dashboard?folder=favorites");
  const favRow = page.locator(`tr:has-text("${subject}")`);
  await expect(favRow).toBeVisible();
  await expect(favRow.locator("button.btn-star")).toContainText("★");

  // 8. Open detail view and unfavorite from inside
  await favRow.locator(".btn-view").click();
  const detailFavBtn = page.locator('button[hx-post*="/toggle-favorite?context=detail"]');
  await expect(detailFavBtn).toContainText("★");
  await detailFavBtn.click();
  await expect(detailFavBtn).toContainText("☆");

  // 9. Go back to Favorites tab -> email is gone from favorites
  await page.goto("/dashboard?folder=favorites");
  await expect(page.locator(`tr:has-text("${subject}")`)).toHaveCount(0);
});
