import { chromium, type FullConfig } from "@playwright/test";
import * as fs from "fs";
import * as path from "path";

export default async function globalSetup(config: FullConfig) {
  const baseURL = process.env.E2E_BASE_URL ?? "http://localhost:3001";
  const browser = await chromium.launch();
  const page = await browser.newPage();

  // Ensure storage directories exist
  const authDir = path.join(__dirname, ".auth");
  if (!fs.existsSync(authDir)) {
    fs.mkdirSync(authDir, { recursive: true });
  }

  console.log(`[E2E Setup] Logging in at ${baseURL}/login`);
  await page.goto(`${baseURL}/login`);
  await page.fill('input[name="email"]', process.env.E2E_ADMIN_EMAIL ?? "admin@e2e.test");
  await page.fill('input[name="password"]', process.env.E2E_ADMIN_PASSWORD ?? "e2e-admin-password");
  await page.click('button[type="submit"]');
  await page.waitForURL("**/dashboard");

  await page.context().storageState({ path: "e2e/.auth/admin.json" });

  // Seed one domain + one alias for all subsequent tests to reuse.
  console.log("[E2E Setup] Seeding domains and aliases...");
  await page.goto(`${baseURL}/domains`);
  if (await page.locator('text=e2etest.test').count() === 0) {
    await page.fill('input[name="domain_name"]', "e2etest.test");
    await page.click('button[type="submit"]');
    // Wait for the list to update
    await page.waitForSelector('text=e2etest.test', { timeout: 15_000 });
  }

  await page.goto(`${baseURL}/aliases`);
  if (await page.locator('text=hello@e2etest.test').count() === 0) {
    await page.selectOption('select[name="domain_id"]', { label: "e2etest.test" });
    await page.fill('input[name="custom_subdomain"]', "hello");
    await page.click('button[type="submit"]');
    // Wait for the list to update
    await page.waitForSelector('text=hello@e2etest.test', { timeout: 15_000 });
  }

  console.log("[E2E Setup] Seeding complete!");
  await browser.close();
}
