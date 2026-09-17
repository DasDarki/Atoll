(() => {
  const RULES = __RULES__;
  const PATIENCE = __PATIENCE__;
  const STEP_PATIENCE = __STEP_PATIENCE__;
  const MARK = "__atollConsentAnswered";

  if (window[MARK]) {
    return;
  }
  window[MARK] = true;

  const started = Date.now();
  let working = false;
  let done = false;

  const root = (selector) => {
    if (!selector) {
      return document;
    }
    const host = document.querySelector(selector);
    return host && host.shadowRoot ? host.shadowRoot : null;
  };

  const usable = (element) => {
    if (!element || element.disabled) {
      return false;
    }
    if (element.getClientRects().length === 0) {
      return false;
    }
    const style = (element.ownerDocument.defaultView || window).getComputedStyle(element);
    return style.visibility !== "hidden" && style.display !== "none" && style.pointerEvents !== "none";
  };

  const find = (step) => {
    const where = root(step.root);
    if (!where) {
      return null;
    }
    for (const selector of step.any) {
      let found;
      try {
        found = where.querySelectorAll(selector);
      } catch (error) {
        continue;
      }
      for (const element of found) {
        if (usable(element)) {
          return element;
        }
      }
    }
    return null;
  };

  const sleep = (milliseconds) => new Promise((resume) => setTimeout(resume, milliseconds));

  const waitFor = async (step) => {
    const until = Date.now() + STEP_PATIENCE;
    while (Date.now() < until) {
      const element = find(step);
      if (element) {
        return element;
      }
      await sleep(120);
    }
    return null;
  };

  // Some banners lock the page while they are open and forget to undo it when they go away.
  const freeThePage = () => {
    for (const element of [document.documentElement, document.body]) {
      if (!element) {
        continue;
      }
      const style = window.getComputedStyle(element);
      if (style.overflow === "hidden" || style.position === "fixed") {
        element.style.setProperty("overflow", "auto", "important");
        element.style.setProperty("position", "static", "important");
      }
    }
  };

  const run = async (rule) => {
    for (const step of rule.steps) {
      const element = await waitFor(step);
      if (!element) {
        if (step.optional) {
          continue;
        }
        return false;
      }
      element.click();
      if (step.final) {
        return true;
      }
      await sleep(250);
    }
    return true;
  };

  const look = async () => {
    if (done || working) {
      return;
    }
    working = true;
    try {
      for (const rule of RULES) {
        if (!document.querySelector(rule.detect)) {
          continue;
        }
        if (await run(rule)) {
          done = true;
          await sleep(400);
          freeThePage();
          return;
        }
      }
    } catch (error) {
      // A banner that disappears under our hands is no reason to bother the page.
    } finally {
      working = false;
    }
  };

  const observer = new MutationObserver(() => {
    if (Date.now() - started > PATIENCE) {
      observer.disconnect();
      return;
    }
    look();
  });

  const watch = () => {
    if (document.documentElement) {
      observer.observe(document.documentElement, { childList: true, subtree: true });
      look();
      return;
    }
    requestAnimationFrame(watch);
  };
  watch();

  document.addEventListener("DOMContentLoaded", look);
  window.addEventListener("load", look);
  setTimeout(() => observer.disconnect(), PATIENCE);
})();
