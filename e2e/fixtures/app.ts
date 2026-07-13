import { test as base, expect, type APIRequestContext } from "@playwright/test";
import { clearMailhog } from "../helpers/mailhog";

type AppFixtures = {
  mailhogApi: APIRequestContext;
  cleanMailhog: void;
  seededDomain: string;
  seededAlias: string;
};

export const test = base.extend<AppFixtures>({
  mailhogApi: async ({ request }, use) => {
    await use(request);
  },
  cleanMailhog: async ({ request }, use) => {
    await clearMailhog(request);
    await use();
  },
  seededDomain: async ({}, use) => {
    await use("e2etest.test");
  },
  seededAlias: async ({}, use) => {
    await use("hello@e2etest.test");
  },
});

export { expect };
