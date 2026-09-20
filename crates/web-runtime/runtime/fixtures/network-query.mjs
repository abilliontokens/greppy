import { chromium } from "playwright";

const browser = await chromium.launch();
const page = await browser.newPage();
await page.route("**/ok", (route) =>
  route.fulfill({ body: "ok", contentType: "text/plain", status: 200 }),
);
await page.route("**/missing", (route) =>
  route.fulfill({ body: "missing", contentType: "text/plain", status: 404 }),
);
await page.goto(fixtureUrl + "ok");
await page.goto(fixtureUrl + "missing");
