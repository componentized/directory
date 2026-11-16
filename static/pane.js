// The side pane beside a page's content: taller than the window, it scrolls on its own, and
// along with the page too. As the page scrolls through the content, from where the pane sticks
// under the header to where the content's end reaches the bottom of the window, before the
// footer, the pane scrolls through itself in step, at its start and its end as the page is at
// its. Scrolling the pane itself moves it alone; the page then carries it on from there, the
// difference fading as the page nears either end, so the pane still reaches its ends with the
// content. Content shorter than the pane is followed by space enough for the page to scroll the
// pane to its end before the footer. Without this script, the pane scrolls on its own alone.
//
// The pane's place is worked out afresh for each scroll, from how far through the content the
// page is, rather than stepped along from where it was: a browser that keeps a scroll position in
// whole pixels, e.g. Firefox, would round each step of less than one away.
(() => {
  const pane = document.querySelector(".layout > aside");
  if (!pane) return;
  const layout = pane.parentElement;

  // how far through the content the page is, from 0 to 1, and how far the pane scrolls; `null`
  // when the pane fits, or sits under the content on a narrow screen, with nothing to do
  const measure = () => {
    const most = pane.scrollHeight - pane.clientHeight;
    if (most <= 0 || getComputedStyle(pane).position !== "sticky") return null;
    const top = layout.getBoundingClientRect().top + scrollY;
    const start = top - (parseFloat(getComputedStyle(pane).top) || 0);
    const end = top + layout.offsetHeight - innerHeight;
    if (end <= start) return null;
    const through = Math.min(Math.max((scrollY - start) / (end - start), 0), 1);
    return { through, most };
  };

  // how far the pane was scrolled by hand from where the page puts it, and how far through the
  // content the page was then
  let offset = 0;
  let from = 0;

  // whether the pane's being scrolled by hand: from a wheel or a trackpad, a touch, a key, or its
  // scrollbar, a pointer held down on it, until it has stopped moving, a wheel's smooth scrolling
  // or a trackpad's momentum carrying it on after. The pane's own scroll events alone don't tell,
  // as a browser may report the page's scrolling of it late, e.g. Safari.
  let held = false;
  let until = 0;
  const extend = (ms) => (until = Math.max(until, performance.now() + ms));
  const handled = () => extend(300);
  const byHand = () => held || performance.now() < until;
  for (const type of ["wheel", "touchmove", "keydown"]) {
    pane.addEventListener(type, handled, { passive: true });
  }
  pane.addEventListener("pointerdown", () => (held = true), { passive: true });
  addEventListener("pointerup", () => held && ((held = false), handled()), { passive: true });

  const place = () => {
    // the pane's own, while it's scrolled by hand, rather than the page and the hand at odds
    if (byHand()) return;
    const at = measure();
    if (!at) return;
    const { through, most } = at;
    // the difference fades to none at either end, from all of it where it was made: made at an
    // end, e.g. with the footer in view, it holds there, fading only as the page scrolls away
    const fade =
      through <= from
        ? from > 0
          ? through / from
          : 1
        : from < 1
          ? (1 - through) / (1 - from)
          : 1;
    pane.scrollTop = Math.min(Math.max(through * most + offset * fade, 0), most);
  };

  pane.addEventListener(
    "scroll",
    () => {
      if (!byHand()) return;
      // still moving, by its momentum, say, it's still by hand
      extend(150);
      const at = measure();
      if (!at) return;
      from = at.through;
      offset = pane.scrollTop - at.through * at.most;
    },
    { passive: true },
  );
  addEventListener("scroll", place, { passive: true });

  // content shorter than the pane still scrolls the page as far as the pane scrolls, so the
  // footer comes into view only once both are at their ends: the layout is as tall as the pane
  // needs, the content then followed by space as the pane scrolls on. The pane's own content is
  // measured by its sections, rather than its scroll height, which an open menu adds to.
  const fit = () => {
    layout.style.minHeight = "";
    if (getComputedStyle(pane).position !== "sticky" || !pane.lastElementChild) return;
    const box = pane.getBoundingClientRect();
    const bottom = pane.lastElementChild.getBoundingClientRect().bottom;
    const content = bottom - box.top + pane.scrollTop + parseFloat(getComputedStyle(pane).paddingBottom);
    const most = content - pane.clientHeight;
    if (most <= 0) return;
    const stuck = parseFloat(getComputedStyle(pane).top) || 0;
    layout.style.minHeight = `${Math.ceil(most + innerHeight - stuck)}px`;
  };
  const sized = new ResizeObserver(() => (fit(), place()));
  for (const section of pane.children) sized.observe(section);
  addEventListener("resize", () => (fit(), place()), { passive: true });
  fit();
  // the page may open scrolled part way, e.g. to an anchor or where it was before a reload
  place();
})();
