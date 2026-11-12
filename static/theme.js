// The theme menu: follow the system's color scheme, or pin light or dark. The choice is kept in
// this browser only. An inline script in the page's head applies a pinned theme before it paints,
// and the menu button shows the current choice's icon from CSS.
(() => {
  const root = document.documentElement;
  const button = document.querySelector("[data-theme-button]");
  const menu = document.getElementById("theme-menu");
  if (!button || !menu) return;
  const items = [...menu.querySelectorAll("[data-theme-choice]")];
  const labels = { system: "System", light: "Light", dark: "Dark" };
  const toolbar = { light: "#ffffff", dark: "#1b1d20" };
  const metas = [...document.querySelectorAll('meta[name="theme-color"]')];
  for (const meta of metas) meta.dataset.media = meta.media;

  const stored = () => {
    try {
      const theme = localStorage.getItem("theme");
      return theme === "light" || theme === "dark" ? theme : "system";
    } catch {
      return "system";
    }
  };

  const apply = (theme) => {
    if (theme === "system") delete root.dataset.theme;
    else root.dataset.theme = theme;
    // tint the browser's toolbar to match a pinned theme
    for (const meta of metas) {
      const pinned = theme !== "system";
      meta.media = pinned ? "all" : meta.dataset.media;
      meta.content = pinned
        ? toolbar[theme]
        : meta.dataset.media.includes("dark") ? toolbar.dark : toolbar.light;
    }
    for (const item of items) {
      item.setAttribute("aria-checked", String(item.dataset.themeChoice === theme));
    }
    button.setAttribute("aria-label", `Color theme: ${labels[theme]}`);
    button.title = `Color theme: ${labels[theme]}`;
  };

  const open = (focus) => {
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    const checked = items.find((i) => i.getAttribute("aria-checked") === "true") ?? items[0];
    if (focus === "last") items[items.length - 1].focus();
    else if (focus) checked.focus();
  };
  const close = (refocus) => {
    if (menu.hidden) return;
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    if (refocus) button.focus();
  };

  button.addEventListener("click", () => (menu.hidden ? open(true) : close(false)));
  button.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") (e.preventDefault(), open(true));
    if (e.key === "ArrowUp") (e.preventDefault(), open("last"));
  });
  menu.addEventListener("keydown", (e) => {
    const i = items.indexOf(document.activeElement);
    if (e.key === "ArrowDown") (e.preventDefault(), items[(i + 1) % items.length].focus());
    else if (e.key === "ArrowUp") (e.preventDefault(), items[(i - 1 + items.length) % items.length].focus());
    else if (e.key === "Home") (e.preventDefault(), items[0].focus());
    else if (e.key === "End") (e.preventDefault(), items[items.length - 1].focus());
    else if (e.key === "Escape") (e.preventDefault(), close(true));
    else if (e.key === "Tab") close(false);
  });
  document.addEventListener("click", (e) => {
    if (!button.contains(e.target) && !menu.contains(e.target)) close(false);
  });

  for (const item of items) {
    item.addEventListener("click", () => {
      const theme = item.dataset.themeChoice;
      try {
        if (theme === "system") localStorage.removeItem("theme");
        else localStorage.setItem("theme", theme);
      } catch {}
      apply(theme);
      close(true);
    });
  }
  apply(stored());
})();
