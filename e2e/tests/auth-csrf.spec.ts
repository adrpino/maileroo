import { test, expect } from "../fixtures/app";

test("state-changing POST without CSRF token is rejected", async ({ request }) => {
  const resp = await request.post("/api/v1/emails/send", {
    multipart: {
      from_alias_id: "00000000-0000-0000-0000-000000000000",
      to_email: "x@e2e.test",
      subject: "no csrf",
      body_text: "should fail",
    },
  });
  expect([403, 422, 400]).toContain(resp.status());
});
