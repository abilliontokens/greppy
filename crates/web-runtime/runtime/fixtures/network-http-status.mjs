import { chromium } from "playwright";

const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto(fixtureUrl + "ok");
await page.goto(fixtureUrl + "missing");
// Keep the active page alive for the caller's subsequent web.network read.
