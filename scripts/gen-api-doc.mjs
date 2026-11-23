#!/usr/bin/env node
// Генерирует таблицу эндпоинтов в docs/API.md из docs/openapi.json.
//
// Проверки (A12):
//   * спецификация — OpenAPI 3.1 и содержит paths;
//   * число шаблонов путей совпадает с числом `.route(` в backend/src;
//   * в docs/API.md есть маркеры BEGIN/END GENERATED ENDPOINTS.
//
// Запуск из корня репозитория: node scripts/gen-api-doc.mjs

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const specPath = path.join(root, "docs", "openapi.json");
const apiPath = path.join(root, "docs", "API.md");
const srcPath = path.join(root, "backend", "src");

const BEGIN = "<!-- BEGIN GENERATED ENDPOINTS -->";
const END = "<!-- END GENERATED ENDPOINTS -->";
const METHOD_ORDER = ["get", "post", "put", "patch", "delete", "head", "options"];

function fail(message) {
  console.error(`gen-api-doc: ${message}`);
  process.exit(1);
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    fail(`не удалось прочитать ${path.relative(root, file)}: ${error.message}`);
  }
}

function countRouteRegistrations(dir) {
  let total = 0;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      total += countRouteRegistrations(full);
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      const source = fs.readFileSync(full, "utf8");
      total += source.split(".route(").length - 1;
    }
  }
  return total;
}

function escapeCell(value) {
  return String(value ?? "")
    .replace(/\\/g, "\\\\")
    .replace(/\|/g, "\\|")
    .replace(/\r?\n/g, " ")
    .trim();
}

function authLabel(operation) {
  const security = operation.security ?? [];
  if (security.some((item) => Object.hasOwn(item, "bearerAuth"))) {
    if (operation.__route.includes("/artifacts/")) {
      return "Bearer · launcher.artifacts";
    }
    if (
      operation.__route.includes("/catalog") ||
      operation.__route.includes("/builds")
    ) {
      return "Bearer · launcher.catalog";
    }
    return "Bearer";
  }
  if (security.some((item) => Object.hasOwn(item, "cookieAuth"))) {
    const mutating = ["post", "put", "patch", "delete"].includes(
      operation.__method,
    );
    return mutating ? "Cookie + CSRF" : "Cookie";
  }
  return "—";
}

function renderEndpoints(spec) {
  const paths = spec.paths ?? {};
  const operations = [];
  for (const [route, item] of Object.entries(paths)) {
    for (const [method, operation] of Object.entries(item)) {
      if (!METHOD_ORDER.includes(method)) continue;
      operations.push({
        route,
        method,
        operation: { ...operation, __method: method, __route: route },
        tag: operation.tags?.[0] ?? "Other",
      });
    }
  }

  const tagOrder = (spec.tags ?? []).map((tag) => tag.name);
  const tags = [...new Set(operations.map((item) => item.tag))].sort((a, b) => {
    const left = tagOrder.indexOf(a);
    const right = tagOrder.indexOf(b);
    return (left === -1 ? 999 : left) - (right === -1 ? 999 : right);
  });

  const lines = [
    `> Всего путей: **${Object.keys(paths).length}**, операций: **${operations.length}**.`,
    "> Таблица генерируется из `docs/openapi.json` командой `node scripts/gen-api-doc.mjs`.",
    "",
  ];

  for (const tag of tags) {
    const tagInfo = (spec.tags ?? []).find((item) => item.name === tag);
    lines.push(`### ${tag}${tagInfo?.description ? ` — ${tagInfo.description}` : ""}`);
    lines.push("");
    lines.push("| Метод | Путь | Описание | Доступ |");
    lines.push("|---|---|---|---|");
    const items = operations
      .filter((item) => item.tag === tag)
      .sort((a, b) => {
        if (a.route !== b.route) return a.route.localeCompare(b.route);
        return (
          METHOD_ORDER.indexOf(a.method) - METHOD_ORDER.indexOf(b.method)
        );
      });
    for (const item of items) {
      lines.push(
        `| \`${item.method.toUpperCase()}\` | \`${escapeCell(item.route)}\` | ${escapeCell(item.operation.summary)} | ${authLabel(item.operation)} |`,
      );
    }
    lines.push("");
  }

  return { text: lines.join("\n").trimEnd(), operations };
}

function main() {
  const spec = readJson(specPath);
  if (spec.openapi !== "3.1.0") {
    fail(`ожидался OpenAPI 3.1.0, получено ${spec.openapi ?? "ничего"}`);
  }
  if (!spec.paths || typeof spec.paths !== "object") {
    fail("в спецификации отсутствует paths");
  }

  const specPaths = Object.keys(spec.paths).length;
  const routeCalls = countRouteRegistrations(srcPath);
  if (specPaths !== routeCalls) {
    fail(
      `расхождение: путей в спецификации ${specPaths}, вызовов .route( ${routeCalls}`,
    );
  }

  if (!fs.existsSync(apiPath)) {
    fail(`не найден ${path.relative(root, apiPath)}`);
  }
  const markdown = fs.readFileSync(apiPath, "utf8");
  const start = markdown.indexOf(BEGIN);
  const end = markdown.indexOf(END);
  if (start === -1 || end === -1 || end < start) {
    fail(`в docs/API.md нет маркеров ${BEGIN} / ${END}`);
  }

  const { text, operations } = renderEndpoints(spec);
  const updated =
    markdown.slice(0, start + BEGIN.length) +
    "\n" +
    text +
    "\n" +
    markdown.slice(end);
  fs.writeFileSync(apiPath, updated, "utf8");

  console.log(
    `gen-api-doc: ${specPaths} путей, ${operations.length} операций, .route( ${routeCalls} → docs/API.md`,
  );
}

main();
