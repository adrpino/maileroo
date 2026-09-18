import { test, expect } from "@playwright/test";

const PHONE = { width: 390, height: 844 }; // iPhone 14 logical viewport

test.use({ viewport: PHONE });

test("empty state is not squashed into the checkbox column on mobile", async ({ page }) => {
  await page.goto("/dashboard?q=zzz-no-such-email-zzz");

  const cell = page.locator("#no-emails-placeholder td");
  await expect(cell).toBeVisible();

  const box = (await cell.boundingBox())!;
  console.log(`[empty-state] width=${box.width} height=${box.height} x=${box.x}`);

  // Before the fix this cell inherited grid-area: checkbox and was ~36px wide.
  expect(box.width).toBeGreaterThan(PHONE.width * 0.8);
});

test("label pills stay inside the viewport on mobile", async ({ page }) => {
  await page.goto("/dashboard");

  // Inject a row mirroring email_row.html so the layout rules are exercised
  // regardless of how many emails the seeded database happens to contain.
  await page.locator("#email-table-body").evaluate((tbody) => {
    const pill = (name: string) =>
      `<span class="email-label-tag" style="display: inline-flex; align-items: center; font-size: 0.7rem; font-weight: 600; padding: 1px 6px; border-radius: 9999px; background-color: #2b6cb01a; color: #2b6cb0; border: 1px solid #2b6cb040; max-width: 100px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${name}</span>`;

    tbody.insertAdjacentHTML(
      "afterbegin",
      `<tr id="email-layout-probe">
         <td style="width: 40px; text-align: center;"><input type="checkbox" class="email-checkbox"></td>
         <td class="sender-cell">remitente.muy.largo@ejemplo-de-dominio.com</td>
         <td>
           <span style="font-size: 0.75rem; background: #edf2f7; color: #4a5568; padding: 2px 6px; border-radius: 4px; margin-right: 8px;">hola@e2etest.test</span>
           <span class="subject-text">Un asunto deliberadamente largo que deberia truncarse en pantalla estrecha</span>
           <span class="email-labels-wrapper" style="display: inline-flex; flex-wrap: wrap; gap: 4px; align-items: center; vertical-align: middle; margin-left: 8px; max-width: 260px;">
             ${pill("trabajo")}${pill("importante")}
           </span>
         </td>
         <td class="forwarded-cell desktop-only"></td>
         <td class="date-cell">2026-09-18 15:13</td>
         <td style="text-align: right;"><button class="btn">x</button></td>
       </tr>`,
    );
  });

  const row = page.locator("#email-layout-probe");
  const subject = row.locator(".subject-text");
  const pills = row.locator(".email-label-tag");

  await expect(subject).toBeVisible();
  await expect(pills.first()).toBeVisible();

  const subjectBox = (await subject.boundingBox())!;

  for (const handle of await pills.all()) {
    const box = (await handle.boundingBox())!;
    const text = await handle.innerText();
    console.log(
      `[label:${text.trim()}] x=${box.x.toFixed(0)} right=${(box.x + box.width).toFixed(0)} y=${box.y.toFixed(0)} w=${box.width.toFixed(0)} (viewport=${PHONE.width})`,
    );

    // Before the fix these sat past the end of a clipped, nowrap subject line.
    expect(box.width).toBeGreaterThan(0);
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(PHONE.width);

    // They must sit on their own line, below the subject.
    expect(box.y).toBeGreaterThanOrEqual(subjectBox.y + subjectBox.height - 1);
  }
});
