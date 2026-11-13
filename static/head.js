// Keeps the heights of what's pinned to the top of the window up to date for the page's CSS: the
// site's header, and the breadcrumbs and title of a package, interface or repository page. The
// sidebar, the contents and links to sections keep clear of both. The page head, once the page
// has scrolled under it, is marked as stuck, which draws a line under it.
(() => {
  const root = document.documentElement;
  const header = document.querySelector("header.site");
  const head = document.querySelector("[data-page-head]");

  // a variable is only set when it changes: setting it restyles the page, and Safari on a phone
  // resizes the window as its toolbar shows and hides while scrolling
  const set = (name, value) => {
    if (root.style.getPropertyValue(name) !== value) root.style.setProperty(name, value);
  };
  const measure = () => {
    if (header) set("--site-header", `${header.offsetHeight}px`);
    // a page head is pinned only on a screen with room for it
    const pinned = head && getComputedStyle(head).position === "sticky";
    set("--page-head", pinned ? `${head.offsetHeight}px` : "0px");
    stick();
  };
  const stick = () => {
    if (!head || !header) return;
    const stuck =
      getComputedStyle(head).position === "sticky" &&
      head.getBoundingClientRect().top <= header.offsetHeight + 0.5 &&
      scrollY > 0;
    head.classList.toggle("stuck", stuck);
  };

  const observer = new ResizeObserver(measure);
  if (header) observer.observe(header);
  if (head) observer.observe(head);
  addEventListener("resize", measure, { passive: true });
  addEventListener("scroll", stick, { passive: true });
  measure();
})();
