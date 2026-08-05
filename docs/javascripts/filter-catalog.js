(() => {
  const headers = ["Public call", "Shape", "Output and contract"];

  function enhanceFilterTables(root = document) {
    for (const table of root.querySelectorAll(".md-typeset table")) {
      const cells = [...table.querySelectorAll(":scope > thead > tr > th")];
      const names = cells.map((cell) => cell.textContent.trim());

      if (names.length !== headers.length ||
          !headers.every((header, index) => names[index] === header)) {
        continue;
      }

      table.classList.add("filter-reference");

      for (const row of table.querySelectorAll(":scope > tbody > tr")) {
        [...row.children].forEach((cell, index) => {
          cell.dataset.label = headers[index] ?? "";
        });
      }
    }
  }

  if (typeof document$ !== "undefined") {
    document$.subscribe(() => enhanceFilterTables());
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", () => enhanceFilterTables(), {
      once: true,
    });
  } else {
    enhanceFilterTables();
  }
})();
