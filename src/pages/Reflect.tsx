import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { sankey, sankeyLinkHorizontal, SankeyGraph } from "d3-sankey";
import { planApi, ReflectSummary, errorText, formatMoney, isoDate } from "../api";
import { ErrorBanner } from "../components";

type Preset = "this_month" | "last_month" | "last_3" | "ytd" | "last_12";
const PRESETS: { id: Preset; label: string }[] = [
  { id: "this_month", label: "This month" },
  { id: "last_month", label: "Last month" },
  { id: "last_3", label: "Last 3 months" },
  { id: "ytd", label: "This year" },
  { id: "last_12", label: "Last 12 months" },
];

function presetRange(p: Preset): [string, string] {
  const now = new Date();
  const y = now.getFullYear();
  const m = now.getMonth();
  switch (p) {
    case "this_month":
      return [isoDate(new Date(y, m, 1)), isoDate(new Date(y, m + 1, 0))];
    case "last_month":
      return [isoDate(new Date(y, m - 1, 1)), isoDate(new Date(y, m, 0))];
    case "last_3":
      return [isoDate(new Date(y, m - 2, 1)), isoDate(new Date(y, m + 1, 0))];
    case "ytd":
      return [isoDate(new Date(y, 0, 1)), isoDate(new Date(y, 11, 31))];
    case "last_12":
      return [isoDate(new Date(y, m - 11, 1)), isoDate(new Date(y, m + 1, 0))];
  }
}

/** Groups that have ever had spending take categorical slots in category-list order,
 *  so a group keeps its color whatever date range is shown. Past 8 groups: gray. */
function groupColorMap(order: number[]): Map<number, string> {
  const map = new Map<number, string>();
  order.forEach((id, i) => map.set(id, i < 8 ? `var(--s${i + 1})` : "var(--s-other)"));
  return map;
}

export default function ReflectPage({ goToTransactions }: { goToTransactions: () => void }) {
  const [preset, setPreset] = useState<Preset | null>("this_month");
  const [range, setRange] = useState<[string, string]>(presetRange("this_month"));
  const [data, setData] = useState<ReflectSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    planApi
      .reflect(range[0], range[1])
      .then((d) => {
        setData(d);
        setError(null);
      })
      .catch((e) => setError(errorText(e)));
  }, [range]);

  const colors = useMemo(() => groupColorMap(data?.color_order ?? []), [data]);
  const choose = (p: Preset) => {
    setPreset(p);
    setRange(presetRange(p));
  };

  const net = data ? data.income_cents - data.spending_cents : 0;
  const rate = data && data.income_cents > 0 ? Math.round((net / data.income_cents) * 100) : null;

  return (
    <div>
      <h1>Where did the money go?</h1>
      <div className="filters">
        {PRESETS.map((p) => (
          <button key={p.id} className={preset === p.id ? "chip active" : "chip"} onClick={() => choose(p.id)}>
            {p.label}
          </button>
        ))}
        <input type="date" value={range[0]} onChange={(e) => (setPreset(null), setRange([e.target.value, range[1]]))} />
        <span className="muted">to</span>
        <input type="date" value={range[1]} onChange={(e) => (setPreset(null), setRange([range[0], e.target.value]))} />
      </div>
      <ErrorBanner error={error} />

      {data && (
        <>
          <div className="tiles">
            <div className="tile">
              <div className="tile-label">Came in</div>
              <div className="tile-value">{formatMoney(data.income_cents)}</div>
            </div>
            <div className="tile">
              <div className="tile-label">Went out</div>
              <div className="tile-value">{formatMoney(data.spending_cents)}</div>
            </div>
            <div className="tile">
              <div className="tile-label">{net >= 0 ? "Left over" : "Spent more than came in"}</div>
              <div className="tile-value">{formatMoney(Math.abs(net))}</div>
              {rate != null && net > 0 && <div className="tile-sub">{rate}% of income</div>}
            </div>
          </div>

          {data.uncategorized_count > 0 && (
            <div className="notice">
              {data.uncategorized_count} uncategorized transaction{data.uncategorized_count === 1 ? "" : "s"} (
              {formatMoney(data.uncategorized_out_cents)} out) {data.uncategorized_count === 1 ? "isn't" : "aren't"} included.{" "}
              <button className="link" onClick={goToTransactions}>
                Categorize them
              </button>
            </div>
          )}

          {data.income_cents === 0 && data.spending_cents === 0 ? (
            <p className="empty">No categorized transactions in this date range yet.</p>
          ) : (
            <>
              <h2>Money flow</h2>
              <SankeyChart data={data} colors={colors} />
              <h2>Spending by category</h2>
              <Breakdown data={data} colors={colors} />
            </>
          )}
        </>
      )}
    </div>
  );
}

// ---------- Sankey ----------

interface NodeExtra {
  id: string;
  label: string;
  color: string;
  column: number;
}
interface LinkExtra {
  color: string;
}

const MAX_CATS_PER_GROUP = 6;
/** Groups and categories smaller than this share of the total are folded together
 *  in the chart so it stays readable; the table below still lists every one. */
const FOLD_SHARE = 0.025;

function buildGraph(data: ReflectSummary, colors: Map<number, string>) {
  const nodes: NodeExtra[] = [];
  const links: { source: string; target: string; value: number; color: string }[] = [];
  const neutral = "var(--s-neutral)";
  const hubTotal = Math.max(data.income_cents, data.spending_cents);

  for (const s of data.income_sources) {
    nodes.push({ id: `inc-${s.category_id}`, label: s.name, color: neutral, column: 0 });
    links.push({ source: `inc-${s.category_id}`, target: "hub", value: s.amount_cents, color: neutral });
  }
  if (data.spending_cents > data.income_cents) {
    nodes.push({ id: "from-savings", label: "From savings / other", color: neutral, column: 0 });
    links.push({ source: "from-savings", target: "hub", value: data.spending_cents - data.income_cents, color: neutral });
  }
  nodes.push({ id: "hub", label: data.income_cents > 0 ? "Income" : "Spending", color: neutral, column: 1 });

  const small = hubTotal * FOLD_SHARE;
  const bigGroups = data.spending_groups.filter((g) => g.amount_cents >= small);
  const smallGroups = data.spending_groups.filter((g) => g.amount_cents < small);
  if (smallGroups.length === 1) bigGroups.push(smallGroups.pop()!);
  if (smallGroups.length > 1) {
    const total = smallGroups.reduce((s, g) => s + g.amount_cents, 0);
    nodes.push({ id: "small-groups", label: `${smallGroups.length} smaller groups`, color: "var(--s-other)", column: 2 });
    links.push({ source: "hub", target: "small-groups", value: total, color: "var(--s-other)" });
  }
  for (const g of bigGroups) {
    const color = colors.get(g.group_id) ?? "var(--s-other)";
    const gid = `grp-${g.group_id}`;
    nodes.push({ id: gid, label: g.name, color, column: 2 });
    links.push({ source: "hub", target: gid, value: g.amount_cents, color });
    let shown = g.categories.slice(0, MAX_CATS_PER_GROUP).filter((c) => c.amount_cents >= small);
    if (shown.length === g.categories.length - 1) shown = g.categories.slice(); // don't fold a lone category
    const rest = g.amount_cents - shown.reduce((s, c) => s + c.amount_cents, 0);
    for (const c of shown) {
      nodes.push({ id: `cat-${c.category_id}`, label: c.name, color, column: 3 });
      links.push({ source: gid, target: `cat-${c.category_id}`, value: c.amount_cents, color });
    }
    if (rest > 0) {
      nodes.push({ id: `rest-${g.group_id}`, label: shown.length ? `Other ${g.name}` : g.name, color, column: 3 });
      links.push({ source: gid, target: `rest-${g.group_id}`, value: rest, color });
    }
  }
  if (data.income_cents > data.spending_cents) {
    nodes.push({ id: "saved", label: "Left over", color: "var(--s-saved)", column: 2 });
    links.push({ source: "hub", target: "saved", value: data.income_cents - data.spending_cents, color: "var(--s-saved)" });
  }
  return { nodes, links, hubTotal };
}

function SankeyChart({ data, colors }: { data: ReflectSummary; colors: Map<number, string> }) {
  const wrap = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);
  const [hover, setHover] = useState<{ x: number; y: number; text: string } | null>(null);

  useEffect(() => {
    if (!wrap.current) return;
    const ro = new ResizeObserver(([e]) => setWidth(Math.max(600, e.contentRect.width)));
    ro.observe(wrap.current);
    return () => ro.disconnect();
  }, []);

  const { graph, hubTotal, height } = useMemo(() => {
    const { nodes, links, hubTotal } = buildGraph(data, colors);
    const leafCount = Math.max(
      nodes.filter((n) => n.column === 3).length,
      nodes.filter((n) => n.column === 0).length,
    );
    const height = Math.max(360, Math.min(900, leafCount * 26));
    const layout = sankey<NodeExtra, LinkExtra>()
      .nodeId((n) => n.id)
      .nodeAlign((n) => (n as NodeExtra).column)
      .nodeWidth(12)
      .nodePadding(10)
      .nodeSort(null)
      .extent([
        [1, 8],
        [width - 1, height - 8],
      ]);
    const graph: SankeyGraph<NodeExtra, LinkExtra> = layout({
      nodes: nodes.map((n) => ({ ...n })),
      links: links.map((l) => ({ ...l })),
    });
    return { graph, hubTotal, height };
  }, [data, colors, width]);

  // Leave room for labels: they sit right of each node, except the last column (left of it).
  const pct = (v: number) => (hubTotal > 0 ? ` (${Math.round((v / hubTotal) * 100)}%)` : "");
  const show = (e: React.MouseEvent, text: string) => {
    const r = wrap.current!.getBoundingClientRect();
    setHover({ x: e.clientX - r.left, y: e.clientY - r.top, text });
  };

  return (
    <div className="sankey-wrap" ref={wrap} onMouseLeave={() => setHover(null)}>
      <svg width={width} height={height} role="img" aria-label="Money flow from income to spending categories">
        <g fill="none">
          {graph.links.map((l, i) => {
            const s = l.source as NodeExtra & { x1: number };
            const t = l.target as NodeExtra;
            return (
              <path
                key={i}
                className="sankey-link"
                d={sankeyLinkHorizontal()(l) ?? undefined}
                style={{ stroke: (l as LinkExtra).color }}
                strokeWidth={Math.max(1, l.width ?? 1)}
                onMouseMove={(e) => show(e, `${s.label} → ${t.label}: ${formatMoney(l.value)}${pct(l.value)}`)}
              />
            );
          })}
        </g>
        {graph.nodes.map((n) => {
          const x0 = n.x0 ?? 0,
            x1 = n.x1 ?? 0,
            y0 = n.y0 ?? 0,
            y1 = n.y1 ?? 0;
          const last = n.column === 3 || (n.column === 2 && !graph.nodes.some((m) => m.column === 3));
          const h = y1 - y0;
          return (
            <g key={n.id} onMouseMove={(e) => show(e, `${n.label}: ${formatMoney(n.value ?? 0)}${pct(n.value ?? 0)}`)}>
              <rect x={x0} y={y0} width={x1 - x0} height={Math.max(1, h)} rx={2} style={{ fill: n.color }} />
              {h >= 9 && (
                <text
                  x={last ? x0 - 6 : x1 + 6}
                  y={(y0 + y1) / 2}
                  dy="0.35em"
                  textAnchor={last ? "end" : "start"}
                  className="sankey-label"
                >
                  {n.label} <tspan className="sankey-value">{formatMoney(n.value ?? 0)}</tspan>
                </text>
              )}
            </g>
          );
        })}
      </svg>
      {hover && (
        <div className="tooltip" style={{ left: hover.x + 12, top: hover.y + 12 }}>
          {hover.text}
        </div>
      )}
    </div>
  );
}

// ---------- Breakdown table (also the chart's table view) ----------

function Breakdown({ data, colors }: { data: ReflectSummary; colors: Map<number, string> }) {
  const [open, setOpen] = useState<Set<number>>(new Set());
  const max = Math.max(1, ...data.spending_groups.map((g) => g.amount_cents));
  const toggle = (id: number) => {
    const next = new Set(open);
    next.has(id) ? next.delete(id) : next.add(id);
    setOpen(next);
  };
  return (
    <table className="breakdown">
      <tbody>
        {data.spending_groups.map((g) => (
          <Fragment key={g.group_id}>
            <tr className="group-row clickable" onClick={() => toggle(g.group_id)}>
              <td className="bd-name">
                <span className="caret">{open.has(g.group_id) ? "▾" : "▸"}</span>
                <span className="swatch" style={{ background: colors.get(g.group_id) ?? "var(--s-other)" }} />
                {g.name}
              </td>
              <td className="bd-bar">
                <div className="bar" style={{ width: `${(g.amount_cents / max) * 100}%`, background: colors.get(g.group_id) ?? "var(--s-other)" }} />
              </td>
              <td className="num">{formatMoney(g.amount_cents)}</td>
              <td className="num muted">
                {data.spending_cents > 0 ? `${Math.round((g.amount_cents / data.spending_cents) * 100)}%` : ""}
              </td>
            </tr>
            {open.has(g.group_id) &&
              g.categories.map((c) => (
                <tr key={`c${c.category_id}`}>
                  <td className="cat-name">{c.name}</td>
                  <td className="bd-bar">
                    <div className="bar sub" style={{ width: `${(c.amount_cents / max) * 100}%`, background: colors.get(g.group_id) ?? "var(--s-other)" }} />
                  </td>
                  <td className="num">{formatMoney(c.amount_cents)}</td>
                  <td className="num muted">
                    {data.spending_cents > 0 ? `${Math.round((c.amount_cents / data.spending_cents) * 100)}%` : ""}
                  </td>
                </tr>
              ))}
          </Fragment>
        ))}
      </tbody>
    </table>
  );
}
