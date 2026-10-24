/** Task list, task detail timeline, approvals inbox, and agent registry. */

import * as api from "./api";
import {
  badge,
  emptyState,
  el,
  errorState,
  formatTime,
  icon,
  messageText,
  relativeTime,
  shortId,
  skeletonRows,
  statTile,
  toast,
} from "./dom";
import type {
  AgentRecord,
  ApprovalRecord,
  AuditRecord,
  Decision,
  SearchResult,
  StreamEvent,
  TaskDetail,
  TaskRecord,
} from "./types";

type View =
  | "dashboard"
  | "tasks"
  | "approvals"
  | "agents"
  | "traffic"
  | "search"
  | "audit";

const TASK_STATES = [
  "submitted",
  "input-required",
  "working",
  "completed",
  "failed",
  "rejected",
  "canceled",
];

const PAGE_META: Record<View, { title: string; subtitle: string }> = {
  dashboard: {
    title: "Dashboard",
    subtitle: "Global overview of A2A Warden activity",
  },
  tasks: {
    title: "Tasks",
    subtitle: "Every A2A task observed by the warden, newest first",
  },
  approvals: {
    title: "Approvals",
    subtitle: "Human in the loop: decide which tasks may proceed",
  },
  agents: {
    title: "Agents",
    subtitle: "The registry of agents the warden mediates",
  },
  traffic: {
    title: "Live Traffic",
    subtitle: "Real-time A2A task event stream",
  },
  search: {
    title: "Global Search",
    subtitle: "Search across tasks, audit entries, and approvals",
  },
  audit: {
    title: "Audit Log",
    subtitle: "Hash-chained audit trail of every warden action",
  },
};

const view = document.getElementById("view") as HTMLElement;
const nav = document.getElementById("nav") as HTMLElement;
const navItems = nav.querySelectorAll<HTMLElement>(".nav__item[data-view]");
const pageTitle = document.getElementById("page-title") as HTMLElement;
const pageSubtitle = document.getElementById("page-subtitle") as HTMLElement;
const statusIndicator = document.getElementById("status-indicator") as HTMLElement;
const statusText = statusIndicator.querySelector(".warden-status__text") as HTMLElement;
const approvalBadge = document.getElementById("approval-badge") as HTMLElement;
const menuToggle = document.getElementById("menu-toggle") as HTMLElement;
const sidebar = document.getElementById("sidebar") as HTMLElement;
const sidebarScrim = document.getElementById("sidebar-scrim") as HTMLElement;
const themeToggle = document.getElementById("theme-toggle") as HTMLElement;

let currentView: View = "dashboard";
let currentTaskId: string | null = null;
let taskStatus = "";
let taskQuery = "";
let approvalStatus = "pending";
let approvalQuery = "";
let agentQuery = "";
let activeSearch: HTMLInputElement | null = null;

// Phase 5: live traffic
let trafficEvents: StreamEvent[] = [];
let trafficUnsubscribe: (() => void) | null = null;
let trafficPaused = false;
let taskPage = 1;
let approvalPage = 1;
let searchDebounceTimeout: ReturnType<typeof setTimeout> | null = null;
let searchTaskPage = 1;


// Phase 5: search
// Phase 5: Dashboard - global statistics
let dashboardTasks: TaskRecord[] = [];
let dashboardApprovals: any[] = [];
let dashboardAgents: AgentRecord[] = [];
let dashboardAuditCount = 0;
let dashboardLoading = false;

let searchQuery = "";
let searchResult: SearchResult | null = null;
let searchLoading = false;

// Phase 5: audit
let auditPage = 1;
const auditPerPage = 25;
let searchAuditPage = 1;
let searchApprovalPage = 1;

let auditEntries: AuditRecord[] = [];
let auditVerification: { valid: boolean; entries: number; brokenAt?: number; lastHash?: string } | null = null;
let auditLoading = false;

/* ==========================================================================
   Bootstrap
   ========================================================================== */

/** Wires the navigation and renders the initial view. */
export function bootstrap(): void {
  navItems.forEach((item) => {
    item.addEventListener("click", () => {
      navItems.forEach((navItem) => {
        navItem.classList.remove("is-active");
        navItem.removeAttribute("aria-current");
      });
      item.classList.add("is-active");
      item.setAttribute("aria-current", "page");
      currentView = (item.dataset["view"] as View) ?? "tasks";
      currentTaskId = null;
      activeSearch = null;
      updatePageMeta();
      closeSidebar();
      void render();
    });
  });

  themeToggle.addEventListener("click", () => {
    toggleTheme();
  });

  menuToggle.addEventListener("click", () => {
    if (sidebar.classList.contains("is-open")) {
      closeSidebar();
    } else {
      openSidebar();
    }
  });

  sidebarScrim.addEventListener("click", () => {
    closeSidebar();
  });

  document.addEventListener("keydown", (event) => {
    const search = activeSearch;
    if (event.key === "/" && !isTyping(event.target)) {
      event.preventDefault();
      search?.focus();
      search?.select();
      return;
    }
    if (event.key === "Escape" && search && event.target === search) {
      search.value = "";
      search.dispatchEvent(new Event("input"));
    }
    if (event.key === "Escape") {
      closeSidebar();
    }
  });

  applyStoredTheme();
  updatePageMeta();
  void refreshStatus();
  void render();
  void refreshApprovalBadge();
}

/** Shows the sidebar on narrow screens. */
function openSidebar(): void {
  sidebar.classList.add("is-open");
  sidebarScrim.hidden = false;
  menuToggle.setAttribute("aria-expanded", "true");
}

/** Hides the sidebar on narrow screens. */
function closeSidebar(): void {
  sidebar.classList.remove("is-open");
  sidebarScrim.hidden = true;
  menuToggle.setAttribute("aria-expanded", "false");
}

/** Reads the stored colour scheme and applies it to the document. */
function applyStoredTheme(): void {
  const stored = window.localStorage.getItem("a2a-warden-theme");
  if (stored === "dark" || stored === "light") {
    document.documentElement.dataset["theme"] = stored;
  }
}

/** Switches between the light and the dark colour scheme. */
function toggleTheme(): void {
  const current = document.documentElement.dataset["theme"];
  // Without an explicit attribute the light theme is rendered, so the next
  // click always moves to the dark one.
  const next = current === "dark" ? "light" : "dark";
  document.documentElement.dataset["theme"] = next;
  window.localStorage.setItem("a2a-warden-theme", next);
}

/** Updates the top bar title for the active view. */
function updatePageMeta(): void {
  const meta = PAGE_META[currentView];
  pageTitle.textContent = meta.title;
  pageSubtitle.textContent = meta.subtitle;
}

/** Polls the warden health endpoint. */
export async function refreshStatus(): Promise<void> {
  try {
    const info = await api.health();
    statusText.textContent = `Warden ${info.version}`;
    statusIndicator.className = "warden-status is-ok";
    statusIndicator.title = "The warden is reachable";
  } catch (error) {
    statusText.textContent = "Offline";
    statusIndicator.className = "warden-status is-err";
    statusIndicator.title = (error as Error).message;
  }
}

/** Renders the active view, or the open task when there is one. */
export async function render(): Promise<void> {
  // Stop the live traffic feed when leaving the traffic view.
  if (currentView !== "traffic" && trafficUnsubscribe) {
    trafficUnsubscribe();
    trafficUnsubscribe = null;
  }
  // Do not auto-start traffic feed; user must click Start on Live traffic page.

  view.replaceChildren(skeletonRows(4, 4));
  try {
    if (currentTaskId) {
      await renderTaskDetail(currentTaskId);
      return;
    }
    switch (currentView) {
      case "dashboard":
        await renderDashboard();
        break;
      case "tasks":
        await renderTasks();
        break;
      case "approvals":
        await renderApprovals();
        break;
      case "agents":
        await renderAgents();
        break;
      case "traffic":
        await renderTraffic();
        break;
      case "search":
        await renderSearch();
        break;
      case "audit":
        await renderAudit();
        break;
    }
  } catch (error) {
    view.replaceChildren(errorState(error, () => void render()));
  }
}

/* ==========================================================================
   Shared building blocks
   ========================================================================== */

/** A page header with a title, a subtitle and optional actions. */
function pageHead(
  title: string,
  subtitle: string,
  actions: HTMLElement[] = [],
): HTMLElement {
  return el("div", { class: "page-head" }, [
    el("div", {}, [
      el("h2", { class: "page-head__title" }, [title]),
      el("p", { class: "page-head__subtitle" }, [subtitle]),
    ]),
    actions.length > 0 ? el("div", { class: "row" }, actions) : el("span"),
  ]);
}

/** A search field that filters the results without losing the caret. */
function searchField(
  placeholder: string,
  onInput: (value: string) => void,
): HTMLInputElement {
  const input = el("input", {
    class: "input",
    type: "search",
    placeholder,
    "aria-label": placeholder,
    autocomplete: "off",
  });
  input.addEventListener("input", () => {
    onInput(input.value);
  });
  activeSearch = input;
  return input;
}

/** A dropdown that filters the results. */
function selectField(
  options: { value: string; label: string }[],
  selected: string,
  onChange: (value: string) => void,
): HTMLSelectElement {
  const select = el("select", { class: "select" });
  for (const option of options) {
    const node = el("option", { value: option.value }, [option.label]);
    select.append(node);
  }
  select.value = selected;
  select.addEventListener("change", () => {
    onChange(select.value);
  });
  return select;
}

/** A timestamp rendered as a relative label with the absolute time as tip. */
function timeCell(value: string): HTMLElement {
  return el("span", { title: formatTime(value) }, [relativeTime(value)]);
}

/** A table cell holding a monospace identifier. */
function idCell(value: string, length = 10): HTMLElement {
  return el("code", { class: "cell-id", title: value }, [shortId(value, length)]);
}

/** The trailing chevron of a clickable row. */
function chevronCell(): HTMLElement {
  return el("td", { class: "cell-actions" }, [icon("chevron")]);
}

/** Wraps a table so it can scroll horizontally on narrow screens. */
function table(columns: string[], rows: HTMLElement[]): HTMLElement {
  const head = el(
    "tr",
    {},
    columns.map((column) => el("th", {}, [column])),
  );
  const body = el("tbody", {}, rows);
  return el("div", { class: "table-wrap" }, [
    el("div", { class: "table-scroll" }, [
      el("table", {}, [el("thead", {}, [head]), body]),
    ]),
  ]);
}

/** Makes a row open the given task. */
function openTask(row: HTMLElement, taskId: string): void {
  row.classList.add("is-clickable");
  row.setAttribute("tabindex", "0");
  row.setAttribute("role", "link");
  row.addEventListener("click", () => {
    void renderTaskDetail(taskId);
  });
  row.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      void renderTaskDetail(taskId);
    }
  });
}

/* ==========================================================================
   Dashboard - Global Statistics
   ========================================================================== */


async function loadDashboardData(): Promise<void> {
  dashboardLoading = true;
  try {
    const [tasks, approvals, agents, audit] = await Promise.all([
      api.listTasks({}),
      api.listApprovals(),
      api.listAgents(),
      api.exportAudit().catch(() => [] as any[]),
    ]);
    dashboardTasks = tasks.tasks || [];
    dashboardApprovals = approvals.approvals || [];
    dashboardAgents = agents;
    dashboardAuditCount = audit.length;
  } catch {
    dashboardTasks = [];
    dashboardApprovals = [];
    dashboardAgents = [];
    dashboardAuditCount = 0;
  } finally {
    dashboardLoading = false;
  }
}

async function renderDashboard(): Promise<void> {
  if (dashboardTasks.length === 0 && dashboardApprovals.length === 0 && dashboardAgents.length === 0 && !dashboardLoading) {
    await loadDashboardData();
  }

  if (dashboardLoading) {
    view.replaceChildren(skeletonRows(6, 4));
    return;
  }

  // Compute statistics
  const totalTasks = dashboardTasks.length;
  const workingTasks = dashboardTasks.filter((t) => t.status === "working").length;
  const inputRequiredTasks = dashboardTasks.filter((t) => t.status === "input-required").length;
  const completedTasks = dashboardTasks.filter((t) => t.status === "completed").length;
  const failedTasks = dashboardTasks.filter(
    (t) =>
      t.status === "failed" ||
      t.status === "rejected" ||
      t.status === "canceled",
  ).length;

  const pendingApprovals = dashboardApprovals.filter((a) => a.status === "pending").length;
  const approvedApprovals = dashboardApprovals.filter((a) => a.status === "approved").length;
  const rejectedApprovals = dashboardApprovals.filter((a) => a.status === "rejected").length;
  const cancelledApprovals = dashboardApprovals.filter((a) => a.status === "cancelled").length;

  const totalAgents = dashboardAgents.length;
  const guardedAgents = dashboardAgents.filter((a) => a.requiresApproval).length;
  const allSkills = dashboardAgents.flatMap((a) => {
    try {
      const parsed = JSON.parse(a.skillsJson);
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  });

  // ---- Section stat tiles ----
  const tasksSection = el("div", { class: "card" }, [
    el("div", { class: "card__head" }, [
      icon("tasks"),
      el("h3", {}, ["Tasks"]),
    ]),
    el("div", { class: "card__body" }, [
      el("div", { class: "stats" }, [
        statTile({ label: "total", value: totalTasks, icon: "tasks", tone: "accent" }),
        statTile({ label: "working", value: workingTasks, icon: "activity", tone: "accent" }),
        statTile({ label: "awaiting review", value: inputRequiredTasks, icon: "clock", tone: "warn" }),
        statTile({ label: "completed", value: completedTasks, icon: "check", tone: "ok" }),
        statTile({ label: "failed", value: failedTasks, icon: "alert", tone: "err" }),
      ]),
    ]),
  ]);

  const approvalsSection = el("div", { class: "card" }, [
    el("div", { class: "card__head" }, [
      icon("inbox"),
      el("h3", {}, ["Approvals"]),
    ]),
    el("div", { class: "card__body" }, [
      el("div", { class: "stats" }, [
        statTile({ label: "pending", value: pendingApprovals, icon: "clock", tone: "warn" }),
        statTile({ label: "approved", value: approvedApprovals, icon: "check", tone: "ok" }),
        statTile({ label: "rejected", value: rejectedApprovals, icon: "ban", tone: "err" }),
        statTile({ label: "cancelled", value: cancelledApprovals, icon: "cross", tone: "neutral" }),
      ]),
    ]),
  ]);

  const agentsSection = el("div", { class: "card" }, [
    el("div", { class: "card__head" }, [
      icon("agents"),
      el("h3", {}, ["Agents"]),
    ]),
    el("div", { class: "card__body" }, [
      el("div", { class: "stats" }, [
        statTile({ label: "total", value: totalAgents, icon: "agents", tone: "accent" }),
        statTile({ label: "guarded", value: guardedAgents, icon: "shield", tone: guardedAgents > 0 ? "violet" : "neutral" }),
        statTile({ label: "skills", value: allSkills.length, icon: "zap", tone: "neutral" }),
      ]),
    ]),
  ]);

  const auditSection = el("div", { class: "card" }, [
    el("div", { class: "card__head" }, [
      icon("history"),
      el("h3", {}, ["Audit"]),
    ]),
    el("div", { class: "card__body" }, [
      el("div", { class: "stats" }, [
        statTile({ label: "entries", value: dashboardAuditCount, icon: "history", tone: "neutral" }),
      ]),
    ]),
  ]);

  const refreshBtn = el("button", {
    class: "btn btn--secondary",
    type: "button",
  }, [
    icon("refresh"),
    "Refresh",
  ]);
  refreshBtn.addEventListener("click", async () => {
    dashboardLoading = true;
    void render();
    try {
      await loadDashboardData();
    } finally {
      dashboardLoading = false;
    }
    void render();
  });

  view.replaceChildren(
    pageHead("Dashboard", "Global overview of A2A Warden activity."),
    el("div", { class: "toolbar toolbar--flat" }, [refreshBtn, el("span", { class: "spacer" })]),
    el("div", { class: "stack", style: "gap: 16px;" }, [
      tasksSection,
      approvalsSection,
      agentsSection,
      auditSection,
    ]),
    el("div", { class: "faint", style: "text-align: center; margin-top: 16px;" }, [
      "All statistics are computed from live data. Refresh to update.",
    ]),
  );
}

/* ==========================================================================
   Tasks
   ========================================================================== */

async function renderTasks(): Promise<void> {
  const perPage = 25;
  let totalPages = 1;
  let taskTotal = 0;
  const res = await api.listTasks({ limit: 25, offset: (taskPage - 1) * 25, status: taskStatus || undefined });
  const tasks = res.tasks || [];
  taskTotal = res.total || tasks.length;

  const results = el("div", { class: "stack" });

  const paint = (): void => {
    const query = taskQuery.trim().toLowerCase();
    const filtered = tasks.filter((task) => {
      if (!query || query.length < 3) return true;
      if (taskStatus && task.status !== taskStatus) {
        return false;
      }
      return [task.id, task.agentId, task.contextId, task.status].some((field) =>
        field.toLowerCase().includes(query),
      );
    });

    if (filtered.length === 0) {
      const action = el("button", { class: "btn btn--sm", type: "button" }, [
        "Clear filters",
      ]);
      action.addEventListener("click", () => {
        taskStatus = "";
        taskQuery = "";
        void render();
      });
      results.replaceChildren(
        emptyState({
          icon: "search",
          title: "No tasks match the filters",
          text: "Adjust the search term or the state filter to see more tasks.",
          action,
        }),
      );
      return;
    }

    totalPages = Math.max(1, Math.ceil(taskTotal / perPage));
    if (taskPage > totalPages) taskPage = totalPages;
    (prevBtn as HTMLButtonElement).disabled = taskPage <= 1;
    (nextBtn as HTMLButtonElement).disabled = taskPage >= totalPages;
    pageSpan.textContent = `${taskPage}/${totalPages}`;
    const rows = filtered.map((task) => {
      const row = el("tr", {}, [
        el("td", {}, [el("code", { class: "cell-id", title: task.id }, [task.id])]),
        el("td", {}, [badge(task.status)]),
        el("td", { class: "cell-dim" }, [task.agentId]),
        el("td", { class: "cell-dim" }, [timeCell(task.createdAt)]),
        el("td", { class: "cell-dim" }, [timeCell(task.updatedAt)]),
        chevronCell(),
      ]);
      openTask(row, task.id);
      return row;
    });

    const children: HTMLElement[] = [
      table(["task", "state", "agent", "created", "updated", ""], rows),
    ];
    results.replaceChildren(...children);
  };

  const prevBtn = el("button", { class: "btn btn--sm btn--ghost", type: "button", disabled: "" }, ["← Prev"]);
  const pageSpan = el("span", { class: "faint" }, [`${taskPage}/${totalPages}`]);
  const nextBtn = el("button", { class: "btn btn--sm btn--ghost", type: "button", disabled: "" }, ["Next →"]);
  prevBtn.addEventListener("click", () => { taskPage--; void render(); });
  nextBtn.addEventListener("click", () => { taskPage++; void render(); });

  const taskSearchInput = searchField("Search by task, agent or context…", (value) => {
    taskQuery = value;
    if (searchDebounceTimeout) clearTimeout(searchDebounceTimeout);
    searchDebounceTimeout = setTimeout(() => {
      if (value.trim().length >= 3) { taskPage = 1; void render(); } else { paint(); }
    }, 300);
  });
  taskSearchInput.value = taskQuery;

  const toolbar = el("div", { class: "toolbar toolbar--flat" }, [
    el("div", { class: "field" }, [
      icon("search"),
      taskSearchInput,
    ]),
    selectField(
      [
        { value: "", label: "All states" },
        ...TASK_STATES.map((state) => ({ value: state, label: state })),
      ],
      taskStatus,
      (value) => {
        taskStatus = value;
        taskPage = 1;
        void render();
      },
    ),
    el("span", { class: "spacer" }),
    prevBtn,
    pageSpan,
    nextBtn,
  ]);

  paint();

  view.replaceChildren(
    pageHead(
      "Tasks",
      "Every A2A task observed by the warden, newest first.",
    ),
    toolbar,
    results,
  );
}

/* ==========================================================================
   Task detail
   ========================================================================== */

async function renderTaskDetail(taskId: string): Promise<void> {
  activeSearch = null;
  currentTaskId = taskId;
  view.replaceChildren(skeletonRows(3, 3));
  const detail = await api.getTask(taskId);

  pageTitle.textContent = "Task detail";
  pageSubtitle.textContent = `${detail.task.id} · ${detail.task.agentId}`;

  const back = el("button", { class: "btn btn--sm btn--ghost", type: "button" }, [
    icon("back"),
    "Back to tasks",
  ]);
  back.addEventListener("click", () => {
    currentTaskId = null;
    updatePageMeta();
    void render();
  });

  const head = el("div", { class: "detail-head" }, [
    el("div", {}, [
      el("div", { class: "detail-head__title" }, [
        el("code", {}, [detail.task.id]),
        badge(detail.task.status),
      ]),
      el("div", { class: "chips" }, [
        chip("agents", "agent", detail.task.agentId),
        chip("link", "context", detail.task.contextId),
        chip("clock", "created", formatTime(detail.task.createdAt)),
        chip("history", "updated", formatTime(detail.task.updatedAt)),
      ]),
    ]),
    el("div", { class: "row" }, [back]),
  ]);

  const timeline = el("div", { class: "card" }, [
    el("div", { class: "card__head" }, [
      icon("activity"),
      el("h3", {}, ["Message timeline"]),
      el("span", { class: "spacer" }),
      el("span", { class: "faint" }, [`${detail.messages.length} messages`]),
    ]),
    el("div", { class: "card__body" }, [
      detail.messages.length === 0
        ? emptyState({
            icon: "inboxEmpty",
            title: "No messages yet",
            text: "The task has been registered but no traffic has been captured.",
          })
        : el(
            "div",
            { class: "timeline" },
            detail.messages.map((message) =>
              el(
                "div",
                { class: `tl-item role-${message.role}` },
                [
                  el("span", { class: "tl-avatar" }, [
                    icon(message.role === "user" ? "user" : "agent"),
                  ]),
                  el("div", { class: "tl-card" }, [
                    el("div", { class: "tl-card__head" }, [
                      el("span", { class: "tl-card__role" }, [
                        message.role === "agent"
                          ? `Agent (${detail.task.agentId})`
                          : message.role.charAt(0).toUpperCase() + message.role.slice(1),
                      ]),
                      el("span", { class: "tl-card__id" }, [message.id]),
                      el("span", { class: "spacer" }),
                      el("span", {}, [formatTime(message.createdAt)]),
                    ]),
                    el("div", { class: "tl-card__body" }, [
                      el("pre", {}, [messageText(message.partsJson)]),
                    ]),
                  ]),
                ],
              ),
            ),
          ),
    ]),
  ]);

  const panels: HTMLElement[] = [head, timeline];

  if (detail.approval) {
    panels.push(approvalPanel(detail));
  }

  if (detail.artifacts && detail.artifacts.length > 0) {
    panels.push(
      el("div", { class: "card" }, [
        el("div", { class: "card__head" }, [
          icon("layers"),
          el("h3", {}, ["Artifacts"]),
          el("span", { class: "spacer" }),
          el("span", { class: "faint" }, [`${detail.artifacts.length}`]),
        ]),
        el("div", { class: "card__body stack" }, [
          ...detail.artifacts.map((artifact) =>
            el("div", { class: "artifact" }, [
              el("div", { class: "artifact__name" }, [
                icon("file"),
                artifact.name ?? artifact.artifactId,
              ]),
              el("pre", {}, [messageText(artifact.partsJson)]),
            ]),
          ),
        ]),
      ]),
    );
  }

  // Phase 5: forensic actions
  const actions = el("div", { class: "card" }, [
    el("div", { class: "card__head" }, [
      icon("rotate"),
      el("h3", {}, ["Forensic actions"]),
    ]),
    el("div", { class: "card__body" }, [
      el("p", { class: "faint" }, [
        "Replay the original task message or re-send an individual stored message to the remote agent.",
      ]),
    ]),
    el("div", { class: "row forensic--actions" }, [
      (() => {
        const btn = el("button", {
          class: "btn btn--secondary",
          type: "button",
        }, [
          icon("rotate"),
          "Replay task",
        ]);
        btn.addEventListener("click", async () => {
          void replayTaskAction(detail.task.id, "operator");
        });
        return btn;
      })(),
      el("span", { class: "faint" }, [
        "Re-send message: ",
      ]),
      (() => {
        const select = el("select", { class: "select" });
        for (const message of detail.messages) {
          const opt = el("option", { value: message.id }, [message.id]);
          select.appendChild(opt);
        }
        const btn = el("button", {
          class: "btn btn--ghost btn--sm",
          type: "button",
        }, [
          icon("refresh"),
          "Replay",
        ]);
        btn.addEventListener("click", async () => {
          void resendMessageAction(select.value, "operator");
        });
        return el("span", {}, [select, btn]);
      })(),
    ]),
  ]);
  panels.push(actions);

  if (detail.audit.length > 0) {
    panels.push(
      el("div", { class: "card" }, [
        el("div", { class: "card__head" }, [
          icon("history"),
          el("h3", {}, ["Audit trail"]),
        ]),
        el("div", { class: "card__body" }, [
          el(
            "div",
            { class: "audit-list" },
            detail.audit.map((entry) =>
              el("div", { class: "audit-item" }, [
                el("span", { class: "audit-item__action" }, [entry.action]),
                el("span", { class: "audit-item__actor" }, [entry.actor]),
                el("span", { class: "audit-item__time" }, [formatTime(entry.ts)]),
              ]),
            ),
          ),
        ]),
      ]),
    );
  }

  view.replaceChildren(el("div", { class: "detail" }, panels));
}

/** A labelled chip used in the task header. */
function chip(iconName: string, label: string, value: string): HTMLElement {
  return el("span", { class: "chip" }, [
    icon(iconName),
    `${label} `,
    el("strong", {}, [value]),
  ]);
}

/* ==========================================================================
   Approvals
   ========================================================================== */

async function renderApprovals(): Promise<void> {
  const perPage = 25;
  let totalPages = 1;
  let approvalTotal = 0;
  const approvalsRes = await api.listApprovals({ status: approvalStatus || "pending", limit: 25, offset: (approvalPage - 1) * 25 });
  const approvals = approvalsRes.approvals || [];
  approvalTotal = approvalsRes.total || approvals.length;
  const prevBtn = el("button", { class: "btn btn--sm btn--ghost", type: "button", disabled: "" }, ["← Prev"]);
  const pageSpan = el("span", { class: "faint" }, [`${approvalPage}/${totalPages}`]);
  const nextBtn = el("button", { class: "btn btn--sm btn--ghost", type: "button", disabled: "" }, ["Next →"]);
  prevBtn.addEventListener("click", () => { approvalPage--; void render(); });
  nextBtn.addEventListener("click", () => { approvalPage++; void render(); });

  const counts = {
    pending: approvals.filter((approval) => approval.status === "pending").length,
    approved: approvals.filter((approval) => approval.status === "approved").length,
    rejected: approvals.filter((approval) => approval.status === "rejected").length,
    cancelled: approvals.filter((approval) => approval.status === "cancelled").length,
  };

  updateApprovalBadge(counts.pending);

  const results = el("div", { class: "stack" });

  const paint = (): void => {
    const query = approvalQuery.trim().toLowerCase();
    const filtered = approvals.filter((approval) => {
      if (approvalStatus && approval.status !== approvalStatus) {
        return false;
      }
      if (!query) {
        return true;
      }
      return [approval.id, approval.taskId, approval.decidedBy ?? ""].some((field) =>
        field.toLowerCase().includes(query),
      );
    });

    if (filtered.length === 0) {
      const action = el("button", { class: "btn btn--sm", type: "button" }, [
        "Show all approvals",
      ]);
      action.addEventListener("click", () => {
        approvalStatus = "";
        approvalQuery = "";
        void render();
      });
      results.replaceChildren(
        emptyState({
          icon: "inboxEmpty",
          title: "No approvals to show",
          text: "When a task requires a human decision it lands in this inbox.",
          action,
        }),
      );
      return;
    }

    totalPages = Math.max(1, Math.ceil(approvalTotal / perPage));
    if (approvalPage > totalPages) approvalPage = totalPages;
    const rows = filtered.map((approval) => {
      const row = el("tr", {}, [
        el("td", {}, [el("code", { class: "cell-id", title: approval.id }, [approval.id])]),
        el("td", {}, [badge(approval.status)]),
        el("td", { class: "cell-dim" }, [el("code", { class: "cell-mono", title: approval.taskId }, [approval.taskId])]),
        el("td", { class: "cell-dim" }, [approval.decidedBy ?? "—"]),
        el("td", { class: "cell-dim" }, [timeCell(approval.createdAt)]),
        chevronCell(),
      ]);
      openTask(row, approval.taskId);
      return row;
    });

    (prevBtn as HTMLButtonElement).disabled = approvalPage <= 1;
    (nextBtn as HTMLButtonElement).disabled = approvalPage >= totalPages;
    pageSpan.textContent = `${approvalPage}/${totalPages}`;

    results.replaceChildren(
      table(["approval", "status", "task", "decided by", "created", "expires", ""], rows),
    );
  };

  const approvalSearchInput = searchField("Search by approval, task or approver…", (value) => {
    approvalQuery = value;
    if (searchDebounceTimeout) clearTimeout(searchDebounceTimeout);
    searchDebounceTimeout = setTimeout(() => {
      if (value.trim().length >= 3) { approvalPage = 1; void render(); } else { paint(); }
    }, 300);
  });
  approvalSearchInput.value = approvalQuery;

  const toolbar = el("div", { class: "toolbar toolbar--flat" }, [
    el("div", { class: "field" }, [
      icon("search"),
      approvalSearchInput,
    ]),
    selectField(
      [
        { value: "", label: "All statuses" },
        { value: "pending", label: "pending" },
        { value: "approved", label: "approved" },
        { value: "rejected", label: "rejected" },
        { value: "cancelled", label: "cancelled" },
      ],
      approvalStatus,
      (value) => {
        approvalStatus = value;
        approvalPage = 1;
        void render();
      },
    ),
    el("span", { class: "spacer" }),
    prevBtn,
    pageSpan,
    nextBtn,
  ]);

  paint();

  view.replaceChildren(
    pageHead(
      "Approvals",
      "Human in the loop: decide which tasks may proceed.",
    ),
    toolbar,
    results,
  );
}

/** Shows the number of pending approvals next to the inbox entry. */
async function refreshApprovalBadge(): Promise<void> {
  try {
    const approvals = await api.listApprovals({ status: "pending" });
    updateApprovalBadge(approvals.approvals?.length || 0);
  } catch {
    // The badge is decorative: a failure here must not break the console.
  }
}

/** Updates the counter rendered next to the approvals navigation entry. */
function updateApprovalBadge(count: number): void {
  approvalBadge.textContent = String(count);
  approvalBadge.hidden = count === 0;
}

/* ==========================================================================
   Agents
   ========================================================================== */

async function renderAgents(): Promise<void> {
  const agents = await api.listAgents();

  const results = el("div", { class: "stack" });

  const paint = (): void => {
    const query = agentQuery.trim().toLowerCase();
    const filtered = agents.filter((agent) => {
      if (!query) {
        return true;
      }
      return [agent.name, agent.url, ...parseSkills(agent)].some((field) =>
        field.toLowerCase().includes(query),
      );
    });

    if (filtered.length === 0) {
      results.replaceChildren(
        emptyState({
          icon: "agents",
          title: "No agents registered",
          text: "Agents appear here once they announce themselves to the warden.",
        }),
      );
      return;
    }

    const rows = filtered.map((agent) => {
      const agentSkills = parseSkills(agent);
      const skillTags = el("div", { class: "skill-tags" });
      if (agentSkills.length === 0) {
        skillTags.append(el("span", { class: "faint" }, ["—"]));
      } else {
        for (const skill of agentSkills) {
          const gated = gatedSkills(agent).includes(skill);
          const tag = el("span", { class: gated ? "skill skill--gated" : "skill" }, [
            gated ? `${skill} (gated)` : skill,
          ]);
          if (gated) {
            tag.title = "held for human approval";
          }
          skillTags.append(tag);
        }
      }
      return el("tr", {}, [
        el("td", {}, [
          el("span", { class: "cell-id" }, [
            icon("agent"),
            ` ${agent.name}`,
          ]),
        ]),
        el("td", {}, [
          el("code", { class: "cell-mono truncate", title: agent.url }, [agent.url]),
        ]),
        el("td", {}, [skillTags]),
        el("td", {}, [
          badge(agent.requiresApproval ? "input-required" : "approved"),
        ]),
      ]);
    });

    results.replaceChildren(
      table(["agent", "url", "skills", "interception"], rows),
    );
  };

  const toolbar = el("div", { class: "toolbar toolbar--flat" }, [
    el("div", { class: "field" }, [
      icon("search"),
      searchField("Search agents, urls or skills…", (value) => {
        agentQuery = value;
        paint();
      }),
    ]),
    el("span", { class: "spacer" }),
  ]);

  paint();

  view.replaceChildren(
    pageHead(
      "Agents",
      "The registry of agents the warden mediates.",
    ),
    toolbar,
    results,
  );
}

/* ==========================================================================
   Phase 5: Live traffic view
   ========================================================================== */

async function renderTraffic(): Promise<void> {

  const controls = el("div", { class: "toolbar" }, [
    (() => {
      const btn = el("button", {
        class: `btn ${trafficUnsubscribe ? "btn--danger" : "btn--secondary"}`,
        type: "button",
      }, [
        icon(trafficUnsubscribe ? "cross" : "feed"),
        trafficUnsubscribe ? "Stop feed" : "Start feed",
      ]);
      btn.addEventListener("click", async () => {
        if (trafficUnsubscribe) {
          trafficUnsubscribe();
          trafficUnsubscribe = null;
        } else {
          trafficUnsubscribe = api.trafficStream((event) => {
            if (!trafficPaused) {
              trafficEvents.unshift(event);
              if (trafficEvents.length > 500) {
                trafficEvents.pop();
              }
            }
            void render();
          });
        }
        void render();
      });
      return btn;
    })(),
    (() => {
      const btn = el("button", {
        class: "btn btn--ghost",
        type: "button",
      }, [
        icon(trafficPaused ? "activity" : "cross"),
        trafficPaused ? "Resume" : "Pause",
      ]);
      btn.addEventListener("click", () => {
        trafficPaused = !trafficPaused;
        void render();
      });
      return btn;
    })(),
    (() => {
      const btn = el("button", {
        class: "btn btn--ghost btn--sm",
        type: "button",
      }, [
        icon("refresh"),
        "Clear",
      ]);
      btn.addEventListener("click", () => {
        trafficEvents = [];
        void render();
      });
      return btn;
    })(),
    el("span", { class: "spacer" }),
  ]);
  let eventsList = el("div", { class: "feed-list" });
  if (trafficEvents.length === 0) {
    eventsList.replaceChildren(emptyState({
      icon: "feed",
      title: "No live traffic yet",
      text: "Start the feed to watch A2A task events arrive in real time.",
    }));
  } else {
    for (const event of trafficEvents) {
      eventsList.append(trafficEventRow(event));
    }
  }

  view.replaceChildren(
    pageHead("Live traffic", "Real-time A2A task event stream from the warden."),
    controls,
    eventsList,
  );
}

/** Renders a single traffic event as a row. */
function trafficEventRow(event: StreamEvent): HTMLElement {
  let detail = "";

  if ("Task" in event) {
    
    detail = `Task ${event.Task.id}`;
  } else if ("Message" in event) {
    
    detail = `Message ${event.Message.id} (${event.Message.role})`;
  } else if ("StatusUpdate" in event) {
    
    detail = `Status ${event.StatusUpdate.taskId} → ${event.StatusUpdate.status}`;
  } else if ("ArtifactUpdate" in event) {
    
    detail = `Artifact ${event.ArtifactUpdate.taskId}`;
  } else if ("Error" in event) {
    
    detail = `Error ${event.Error.code}: ${event.Error.message}`;
  }

  const payload = "Task" in event ? event.Task
    : "Message" in event ? event.Message
    : "StatusUpdate" in event ? event.StatusUpdate
    : "ArtifactUpdate" in event ? event.ArtifactUpdate
    : "Error" in event ? event.Error
    : null;

  return el("tbody", {}, [
    el("tr", {}, [
      el("td", { colspan: "2" }, [el("code", { class: "cell-mono" }, [`${detail} - ${formatTime(new Date().toISOString())}`])]),
    ]),
    el("tr", {}, [
      el("td", { colspan: "2" }, [el("pre", { style: "margin:0;font-size:11px;" }, [JSON.stringify(payload, null, 2)])]),
    ]),
  ]);
}

/* ==========================================================================
   Phase 5: Global search
   ========================================================================== */

async function renderSearch(): Promise<void> {

  const toolbar = el("div", { class: "toolbar toolbar--flat" }, [
    el("div", { class: "field" }, [
      icon("search"),
      (() => {
        const input = el("input", {
          class: "input",
          type: "search",
          placeholder: "Search tasks, audit entries, approvals…",
          value: searchQuery,
          "aria-label": "Global search",
        });
        input.addEventListener("input", (event) => {
          const target = event.target as HTMLInputElement;
          searchQuery = target.value;
        });
        return input;
      })(),
      (() => {
        const btn = el("button", {
          class: "btn btn--primary",
          type: "button",
        }, [
          icon("search"),
          "Search",
        ]);
        btn.addEventListener("click", async () => {
        if (!searchQuery.trim()) {
          return;
        }
        searchLoading = true;
        void render();
        try {
          searchResult = await api.search(searchQuery);
        } catch (error) {
          view.replaceChildren(errorState(error, () => void render()));
          return;
        } finally {
          searchLoading = false;
        }
        void render();
      });
      return btn;
    })(),
    ]),
    el("span", { class: "spacer" }),
  ]);

  let results: HTMLElement;
  if (searchLoading) {
    results = skeletonRows(5, 3);
  } else if (!searchResult) {
    results = emptyState({
      icon: "search",
      title: "No search yet",
      text: "Enter a query and press Search to look across tasks, audit entries, and approvals.",
    });
  } else if (
    searchResult.tasks.length === 0 &&
    searchResult.audit.length === 0 &&
    searchResult.approvals.length === 0
  ) {
    const action = el("button", { class: "btn btn--sm", type: "button" }, [
      "New search",
    ]);
    action.addEventListener("click", () => {
      searchResult = null;
      void render();
    });
    results = emptyState({
      icon: "search",
      title: "No results found",
      text: `No tasks, audit entries, or approvals match “${searchQuery}”.`,
      action,
    });
  } else {
    results = el("div", { class: "stack" });
    const sections: HTMLElement[] = [];

    if (searchResult.tasks.length > 0) {
      sections.push(
        el("div", { class: "card" }, [
          el("div", { class: "card__head" }, [
            icon("tasks"),
            el("h3", {}, [`Tasks (${searchResult.tasks.length})`]),
          ]),
          el("div", { class: "card__body" }, [
            table(
              ["task", "agent", "status", "created"],
              (searchResult.tasks.slice((searchTaskPage - 1) * 25, searchTaskPage * 25)).map((task) => {
                const row = el("tr", {}, [
                  el("td", {}, [idCell(task.id)]),
                  el("td", { class: "cell-dim" }, [task.agentId]),
                  el("td", {}, [badge(task.status)]),
                  el("td", { class: "cell-dim" }, [timeCell(task.createdAt)]),
                ]);
                openTask(row, task.id);
                return row;
              }),
            ),
          ]),
        ]),
      );
    }

    if (searchResult.audit.length > 0) {
      sections.push(
        el("div", { class: "card" }, [
          el("div", { class: "card__head" }, [
            icon("history"),
            el("h3", {}, [`Audit entries (${searchResult.audit.length})`]),
          ]),
          el("div", { class: "card__body" }, [
            table(
              ["time", "actor", "action", "task"],
              (searchResult.audit.slice((searchAuditPage - 1) * 25, searchAuditPage * 25)).map((entry) => el("tr", {}, [
                el("td", { class: "cell-dim" }, [timeCell(entry.ts)]),
                el("td", {}, [entry.actor]),
                el("td", {}, [entry.action]),
                el("td", { class: "cell-dim" }, [entry.taskId ? entry.taskId : "—"]),
              ])),
            ),
          ]),
        ]),
      );
    }

    if (searchResult.approvals.length > 0) {
      sections.push(
        el("div", { class: "card" }, [
          el("div", { class: "card__head" }, [
            icon("inbox"),
            el("h3", {}, [`Approvals (${searchResult.approvals.length})`]),
          ]),
          el("div", { class: "card__body" }, [
            table(
              ["approval", "task", "status", "decided by"],
              (searchResult.approvals.slice((searchApprovalPage - 1) * 25, searchApprovalPage * 25)).map((approval) => el("tr", {}, [
                el("td", {}, [idCell(approval.id)]),
                el("td", { class: "cell-dim" }, [idCell(approval.taskId, 12)]),
                el("td", {}, [badge(approval.status)]),
                el("td", { class: "cell-dim" }, [approval.decidedBy ?? "—"]),
              ])),
            ),
          ]),
        ]),
      );
    }

    results.replaceChildren(...sections);
  }

  view.replaceChildren(
    pageHead("Global search", "Search across tasks, audit entries, and approvals."),
    toolbar,
    results,
  );
}

/* ==========================================================================
   Phase 5: Audit log
   ========================================================================== */

async function renderAudit(): Promise<void> {

  const prevAuditBtn = el("button", { class: "btn btn--ghost btn--sm", type: "button", disabled: "" }, ["← Prev"]);
  const auditPageSpan = el("span", { class: "faint" }, [`Page ${auditPage}/${Math.max(1, Math.ceil(auditEntries.length / auditPerPage))}`]);
  const nextAuditBtn = el("button", { class: "btn btn--ghost btn--sm", type: "button", disabled: "" }, ["Next →"]);
  prevAuditBtn.addEventListener("click", () => { auditPage--; void render(); });
  nextAuditBtn.addEventListener("click", () => { auditPage++; void render(); });
  const toolbar = el("div", { class: "toolbar toolbar--flat" }, [
    (() => {
      const btn = el("button", {
        class: "btn btn--secondary",
        type: "button",
      }, [
        icon("refresh"),
        "Reload",
      ]);
      btn.addEventListener("click", async () => {
        await loadAudit();
        void render();
      });
      return btn;
    })(),
    (() => {
      const btn = el("button", {
        class: "btn btn--primary",
        type: "button",
      }, [
        icon("shield"),
        "Verify chain",
      ]);
      btn.addEventListener("click", async () => {
        auditLoading = true;
        void render();
        try {
          auditVerification = await api.verifyAudit();
        } catch (error) {
          toast("err", `Verification failed: ${(error as Error).message}`);
        } finally {
          auditLoading = false;
        }
        void render();
      });
      return btn;
    })(),
    (() => {
      const btn = el("button", {
        class: "btn btn--ghost btn--sm",
        type: "button",
      }, [
        icon("copy"),
        "Export",
      ]);
      btn.addEventListener("click", async () => {
        try {
          const entries = await api.exportAudit();
          const blob = new Blob([JSON.stringify(entries, null, 2)], { type: "application/json" });
          const url = URL.createObjectURL(blob);
          const anchor = document.createElement("a");
          anchor.href = url;
          anchor.download = `audit-${new Date().toISOString().slice(0, 10)}.json`;
          document.body.append(anchor);
          anchor.click();
          anchor.remove();
          URL.revokeObjectURL(url);
          toast("ok", `Exported ${entries.length} audit entries.`);
        } catch (error) {
          toast("err", `Export failed: ${(error as Error).message}`);
        }
      });
      return btn;
    })(),
    prevAuditBtn,
    auditPageSpan,
    nextAuditBtn,
    el("span", { class: "spacer" }),
  ]);

  if (auditEntries.length === 0 && !auditLoading) {
    await loadAudit();
  }

  let entriesView: HTMLElement;
  if (auditLoading) {
    entriesView = skeletonRows(6, 4);
  } else if (auditEntries.length === 0) {
    entriesView = emptyState({
      icon: "history",
      title: "No audit entries",
      text: "The audit log is empty. Traffic and decisions appear here as they occur.",
    });
  } else {
    const sortedAudit = [...auditEntries].sort((a, b) => b.ts.localeCompare(a.ts));
    const totalAuditPages = Math.max(1, Math.ceil(sortedAudit.length / auditPerPage));
    if (auditPage > totalAuditPages) auditPage = totalAuditPages;
    const start = (auditPage - 1) * auditPerPage;
    const pageAudit = sortedAudit.slice(start, start + auditPerPage);
    (prevAuditBtn as HTMLButtonElement).disabled = auditPage <= 1;
    (nextAuditBtn as HTMLButtonElement).disabled = auditPage >= totalAuditPages;
    auditPageSpan.textContent = `Page ${auditPage}/${totalAuditPages}`;
    entriesView = table(
      ["time", "actor", "action", "task", "hash"],
      pageAudit.map((entry) => el("tr", {}, [
        el("td", { class: "cell-dim" }, [timeCell(entry.ts)]),
        el("td", {}, [entry.actor]),
        el("td", {}, [entry.action]),
        el("td", { class: "cell-dim" }, [entry.taskId ? shortId(entry.taskId, 12) : "—"]),
        el("td", {}, [
          el("code", { class: "cell-mono", title: entry.hash ?? "" }, [
            shortId(entry.hash ?? "—", 10),
          ]),
        ]),
      ])),
    );
  }

  let chainView: HTMLElement = el("div", { style: "display: none" });
  if (auditVerification) {
    chainView = el("div", { class: "card" }, [
      el("div", { class: "card__head" }, [
        icon("shield"),
        el("h3", {}, ["Hash chain verification"]),
      ]),
      el("div", { class: "card__body" }, [
        el("p", {}, [
          `Verified ${auditVerification.entries} entries. Chain is ` +
          (auditVerification.valid ? "intact" : `broken at index ${auditVerification.brokenAt ?? "?"}`) +
          `.`,
        ]),
      ]),
    ]);
  }

  view.replaceChildren(
    pageHead("Audit log", "Hash-chained audit trail of every warden action."),
    toolbar,
    chainView,
    entriesView,
  );
}
/** Loads audit entries from the API. */
async function loadAudit(): Promise<void> {
  auditLoading = true;
  try {
    auditEntries = await api.exportAudit();
  } catch {
    auditEntries = [];
  } finally {
    auditLoading = false;
  }
}

/* ==========================================================================
   Phase 5: forensic replay / resend helpers
   ========================================================================== */

async function replayTaskAction(taskId: string, actor: string): Promise<void> {
  const buttons = document.querySelectorAll("button");
  for (const button of buttons) {
    (button as HTMLButtonElement).disabled = true;
  }
  try {
    await api.replayTask(taskId, actor);
    toast("ok", "Task replayed successfully.");
    await render();
  } catch (error) {
    toast("err", `Replay failed: ${(error as Error).message}`);
  } finally {
    for (const button of buttons) {
      (button as HTMLButtonElement).disabled = false;
    }
  }
}

async function resendMessageAction(messageId: string, actor: string): Promise<void> {
  const buttons = document.querySelectorAll("button");
  for (const button of buttons) {
    (button as HTMLButtonElement).disabled = true;
  }
  try {
    await api.resendMessage(messageId, actor);
    toast("ok", "Message re-sent successfully.");
    await render();
  } catch (error) {
    toast("err", `Re-send failed: ${(error as Error).message}`);
  } finally {
    for (const button of buttons) {
      (button as HTMLButtonElement).disabled = false;
    }
  }
}

/** Reads the skill list of an agent, tolerating malformed payloads. */
/** Reads the skill list of an agent, tolerating malformed payloads. */
function parseSkills(agent: AgentRecord): string[] {
  try {
    const parsed = JSON.parse(agent.skillsJson) as unknown;
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((skill): skill is string => typeof skill === "string");
  } catch {
    return [];
  }
}

/** Reads the skills of an agent that are held for human approval. */
function gatedSkills(agent: AgentRecord): string[] {
  return agent.requireApprovalSkills ?? [];
}

/* ==========================================================================
   Approval decision
   ========================================================================== */

function approvalPanel(detail: TaskDetail): HTMLElement {
  const approval = detail.approval as ApprovalRecord;
  const pending = approval.status === "pending";

  const head = el("div", { class: "decision__head" }, [
    icon(pending ? "clock" : "check"),
    el("h3", {}, ["Human decision"]),
    badge(approval.status),
  ]);

  const meta = el("p", { class: "decision__meta" }, [
    approval.decidedBy
      ? `Decided by ${approval.decidedBy} on ${formatTime(approval.decidedAt ?? "")}.`
      : "Waiting for a supervisor to decide whether this task may proceed.",
  ]);

  const children: (Node | string)[] = [head, meta];

  if (approval.reason) {
    children.push(
      el("p", { class: "decision__meta" }, [
        el("strong", {}, ["Recorded reason: "]),
        approval.reason,
      ]),
    );
  }

  if (pending) {
    const reason = el("textarea", {
      class: "input",
      rows: "2",
      placeholder: "Optional reason, stored with the decision…",
      "aria-label": "Decision reason",
    });

    const errorBox = el("div", { class: "decision__error" }, [
      icon("alert"),
      el("span", { class: "decision__error-text" }, [""]),
    ]);
    errorBox.hidden = true;

    const actions = el("div", { class: "decision__actions" });

    const makeAction = (
      decision: Decision,
      label: string,
      className: string,
      iconName: string,
    ): void => {
      const button = el("button", { class: `btn ${className}`, type: "button" }, [
        icon(iconName),
        label,
      ]);
      button.addEventListener("click", async () => {
        const buttons = actions.querySelectorAll("button");
        for (const other of buttons) {
          (other as HTMLButtonElement).disabled = true;
        }
        button.classList.add("is-busy");
        try {
          await api.decide(approval.id, decision, reason.value.trim() || undefined);
          toast("ok", `Task ${decision === "approve" ? "approved" : decision === "reject" ? "rejected" : "cancelled"}.`);
          await render();
        } catch (error) {
          errorBox.hidden = false;
          const text = errorBox.querySelector(".decision__error-text");
          if (text) {
            text.textContent = (error as Error).message;
          }
          button.classList.remove("is-busy");
          for (const other of buttons) {
            (other as HTMLButtonElement).disabled = false;
          }
        }
      });
      actions.append(button);
    };

    makeAction("approve", "Approve", "btn--success", "check");
    makeAction("reject", "Reject", "btn--danger", "ban");
    makeAction("cancel", "Cancel", "btn--danger", "cross");

    children.push(
      el("div", { class: "decision__form" }, [
        el("div", {}, [el("label", { class: "label" }, ["Reason"]), reason]),
        actions,
        errorBox,
      ]),
    );
  }

  return el(
    "div",
    { class: `decision${pending ? "" : " is-resolved"}` },
    children,
  );
}

/* ==========================================================================
   Utilities
   ========================================================================== */

/** True when the event target is a text entry, so shortcuts must not fire. */
function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tag = target.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select";
}