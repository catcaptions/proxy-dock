import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { apiGet } from "../api";
import type { UsageGroup } from "../api";
import { PROVIDERS, PROVIDER_SERIES_COLORS, formatDayShort } from "../data";

/**
 * Daily estimated cost per provider, rendered like the t3code usage chart.
 *
 * The curve math (shape-preserving monotone cubic smoothing + 1/2/5
 * nice-scale rounding the maximum *up*) is adapted from
 * `apps/web/src/components/usage/UsageProviderChart.tsx` in
 * pingdotgg/t3code (MIT). Light-mode styling below is proxy-dock's own:
 * light gridlines/axis text, white hover tooltip, per-slug palette.
 */

// ---------------------------------------------------------------------------
// Curve + scale math (t3code-derived, see header).
// ---------------------------------------------------------------------------

const VIEW_WIDTH = 960;
/** 40% taller than the original 260 so the home-lower row (graph height =
 * combined cost-by-provider + breakdown height) gets more presence. */
const VIEW_HEIGHT = 364;
const TICK_COUNT = 4;
/** Headroom above the top gridline so a peak stroke is never clipped. */
const PLOT_TOP = 11;

interface Point {
  readonly x: number;
  readonly y: number;
}

/** Shape-preserving cubic tangents that cannot overshoot spiky usage data. */
function monotoneTangents(points: readonly Point[]): readonly number[] {
  const count = points.length;
  if (count < 2) return [0];

  const slopes: number[] = [];
  for (let index = 0; index < count - 1; index += 1) {
    const dx = (points[index + 1]?.x ?? 0) - (points[index]?.x ?? 0);
    const dy = (points[index + 1]?.y ?? 0) - (points[index]?.y ?? 0);
    slopes.push(dx === 0 ? 0 : dy / dx);
  }

  const tangents: number[] = Array.from({ length: count }, () => 0);
  tangents[0] = slopes[0] ?? 0;
  tangents[count - 1] = slopes[count - 2] ?? 0;
  for (let index = 1; index < count - 1; index += 1) {
    const previous = slopes[index - 1] ?? 0;
    const next = slopes[index] ?? 0;
    tangents[index] = previous * next <= 0 ? 0 : (previous + next) / 2;
  }

  for (let index = 0; index < count - 1; index += 1) {
    const slope = slopes[index] ?? 0;
    if (slope === 0) {
      tangents[index] = 0;
      tangents[index + 1] = 0;
      continue;
    }
    const a = (tangents[index] ?? 0) / slope;
    const b = (tangents[index + 1] ?? 0) / slope;
    const magnitude = a * a + b * b;
    if (magnitude > 9) {
      const scale = 3 / Math.sqrt(magnitude);
      tangents[index] = scale * a * slope;
      tangents[index + 1] = scale * b * slope;
    }
  }

  return tangents;
}

interface CurveSegment {
  readonly from: Point;
  readonly c1: Point;
  readonly c2: Point;
  readonly to: Point;
}

function smoothCurve(points: readonly Point[]): readonly CurveSegment[] {
  if (points.length < 2) return [];
  const tangents = monotoneTangents(points);
  const segments: CurveSegment[] = [];

  for (let index = 0; index < points.length - 1; index += 1) {
    const from = points[index];
    const to = points[index + 1];
    if (from === undefined || to === undefined) continue;
    const dx = to.x - from.x;
    segments.push({
      from,
      c1: { x: from.x + dx / 3, y: from.y + ((tangents[index] ?? 0) * dx) / 3 },
      c2: { x: to.x - dx / 3, y: to.y - ((tangents[index + 1] ?? 0) * dx) / 3 },
      to,
    });
  }
  return segments;
}

function curvePath(segments: readonly CurveSegment[]): string {
  const first = segments[0];
  if (first === undefined) return "";
  let path = `M${first.from.x.toFixed(2)},${first.from.y.toFixed(2)}`;
  for (const segment of segments) {
    path += ` C${segment.c1.x.toFixed(2)},${segment.c1.y.toFixed(2)} ${segment.c2.x.toFixed(2)},${segment.c2.y.toFixed(2)} ${segment.to.x.toFixed(2)},${segment.to.y.toFixed(2)}`;
  }
  return path;
}

/**
 * Scale whose maximum is a readable 1/2/5 x 10^n step at or above the peak.
 * Rounding the maximum *up* is the point: stopping at the last step below
 * the peak leaves the tallest day drawn past the top of the plot, clipped.
 */
function niceScale(peak: number, count: number): { max: number; ticks: readonly number[] } {
  if (peak <= 0) return { max: 0, ticks: [0] };

  const rawStep = peak / count;
  const magnitude = 10 ** Math.floor(Math.log10(rawStep));
  const normalized = rawStep / magnitude;
  const step = (normalized > 5 ? 10 : normalized > 2 ? 5 : normalized > 1 ? 2 : 1) * magnitude;

  const max = Math.ceil(peak / step) * step;
  const ticks: number[] = [];
  for (let value = 0; value <= max + step * 1e-6; value += step) ticks.push(value);
  return { max, ticks };
}

// ---------------------------------------------------------------------------
// Light-mode presentation (proxy-dock's own).
// ---------------------------------------------------------------------------

const GRID = "#e2e8f0";
const AXIS_TEXT = "#64748b";
const HOVER_LINE = "#94a3b8";
/** Rendered plot height — 40% taller than the original 224 (matches VIEW_HEIGHT scale). */
const PLOT_HEIGHT = 314;

const axisUsd = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

/** Sub-cent costs need 4 decimals or everything reads $0.00. */
function fmtCost(value: number): string {
  return value < 0.01 && value > 0 ? `$${value.toFixed(4)}` : axisUsd.format(value);
}

function dayRange(sinceISO: string): string[] {
  const days: string[] = [];
  const start = new Date(sinceISO);
  start.setUTCHours(0, 0, 0, 0);
  const today = new Date();
  today.setUTCHours(0, 0, 0, 0);
  for (let d = new Date(start); d <= today; d.setUTCDate(d.getUTCDate() + 1)) {
    days.push(d.toISOString().slice(0, 10));
  }
  return days;
}

interface DayColumn {
  readonly bands: readonly { readonly slug: string; readonly value: number }[];
  readonly total: number;
}

interface BuiltSeries {
  readonly slug: string;
  readonly label: string;
  readonly color: string;
  readonly total: number;
  /** Whether any day in range carried a priced value (vs. all zero-filled). */
  readonly hasData: boolean;
  readonly area: string;
  readonly line: string;
}

type ChartState =
  | { state: "loading" }
  | { state: "ready"; series: BuiltSeries[]; days: string[]; columns: DayColumn[]; ticks: readonly number[]; max: number }
  | { state: "error"; message: string };

/**
 * Daily estimated cost per provider over time (SVG, no chart dependency).
 * Every point comes from `GET /api/usage/summary?provider=&group_by=day`.
 * Like the reference, each series is absolute (overlapped, measured from
 * zero — never stacked), days without priced usage are zeros so the lines
 * stay continuous, and the scale tops out at the largest single
 * provider-day rather than the combined sum.
 */
export default function DailyCostChart({ since }: { since: string }) {
  const [value, setValue] = useState<ChartState>({ state: "loading" });
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const plotRef = useRef<HTMLDivElement | null>(null);
  const tooltipRef = useRef<HTMLDivElement | null>(null);
  const hoverPositionRef = useRef<{ x: number; y: number } | null>(null);

  useEffectFetch(setValue, since);

  const toY = useMemo(() => {
    if (value.state !== "ready") return (_v: number) => VIEW_HEIGHT;
    const max = value.max;
    return (v: number) =>
      max === 0 ? VIEW_HEIGHT : VIEW_HEIGHT - (v / max) * (VIEW_HEIGHT - PLOT_TOP);
  }, [value]);

  const stepX = value.state === "ready" && value.days.length > 1 ? VIEW_WIDTH / (value.days.length - 1) : 0;

  const positionTooltip = useCallback(() => {
    const plot = plotRef.current;
    const tooltip = tooltipRef.current;
    const hoverPosition = hoverPositionRef.current;
    if (plot === null || tooltip === null || hoverPosition === null) return;

    const gap = 12;
    const tooltipWidth = tooltip.offsetWidth;
    const tooltipHeight = tooltip.offsetHeight;
    const plotWidth = plot.clientWidth;
    const plotHeight = plot.clientHeight;
    const preferredLeft =
      hoverPosition.x + gap + tooltipWidth <= plotWidth
        ? hoverPosition.x + gap
        : hoverPosition.x - gap - tooltipWidth;
    const preferredTop =
      hoverPosition.y + gap + tooltipHeight <= plotHeight
        ? hoverPosition.y + gap
        : hoverPosition.y - gap - tooltipHeight;
    const left = Math.min(Math.max(0, preferredLeft), Math.max(0, plotWidth - tooltipWidth));
    const top = Math.min(Math.max(0, preferredTop), Math.max(0, plotHeight - tooltipHeight));
    plot.style.setProperty("--daily-tooltip-left", `${left}px`);
    plot.style.setProperty("--daily-tooltip-top", `${top}px`);
  }, []);

  useLayoutEffect(() => {
    if (hoverIndex === null) return;
    positionTooltip();

    const plot = plotRef.current;
    const tooltip = tooltipRef.current;
    if (plot === null || tooltip === null || typeof ResizeObserver === "undefined") return;

    const observer = new ResizeObserver(positionTooltip);
    observer.observe(plot);
    observer.observe(tooltip);
    return () => observer.disconnect();
  }, [hoverIndex, positionTooltip]);

  const handleMove = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const plot = plotRef.current;
      if (plot === null || value.state !== "ready" || value.days.length === 0) return;
      const bounds = plot.getBoundingClientRect();
      if (bounds.width === 0) return;
      const localX = Math.min(bounds.width, Math.max(0, event.clientX - bounds.left));
      const localY = Math.min(bounds.height, Math.max(0, event.clientY - bounds.top));
      const fraction = localX / bounds.width;
      const index = Math.round(fraction * (value.days.length - 1));
      hoverPositionRef.current = { x: localX, y: localY };
      positionTooltip();
      setHoverIndex(Math.min(value.days.length - 1, Math.max(0, index)));
    },
    [positionTooltip, value],
  );

  const hoveredDay = value.state === "ready" && hoverIndex !== null ? value.days[hoverIndex] : undefined;
  const hoveredColumn = value.state === "ready" && hoverIndex !== null ? value.columns[hoverIndex] : undefined;

  return (
    <section className="card" aria-label="Daily cost" data-testid="daily-cost-chart">
      <h2>Daily cost</h2>
      {value.state === "loading" ? <p className="muted">Loading…</p> : null}
      {value.state === "error" ? <p className="error">{value.message}</p> : null}
      {value.state === "ready" && value.series.every((s) => !s.hasData) ? (
        <p className="muted" data-testid="daily-cost-empty">
          No priced usage in this range yet — costs appear here as signed-in requests flow.
        </p>
      ) : null}
      {value.state === "ready" ? (
        <>
          <div style={{ display: "flex", gap: 8 }}>
            {/* Axis labels sit outside the plot so they stay aligned to gridlines. */}
            <div style={{ position: "relative", height: PLOT_HEIGHT, width: 64, flex: "none" }}>
              {value.ticks.map((tick) => (
                <span
                  key={tick}
                  style={{
                    position: "absolute",
                    right: 0,
                    transform: "translateY(-50%)",
                    top: `${(toY(tick) / VIEW_HEIGHT) * 100}%`,
                    fontSize: 11,
                    color: AXIS_TEXT,
                    fontVariantNumeric: "tabular-nums",
                    whiteSpace: "nowrap",
                  }}
                >
                  {tick === 0 ? "0" : axisUsd.format(tick)}
                </span>
              ))}
            </div>

            <div
              ref={plotRef}
              style={{ position: "relative", height: PLOT_HEIGHT, flex: 1, minWidth: 0 }}
              onMouseMove={handleMove}
              onMouseLeave={() => {
                hoverPositionRef.current = null;
                setHoverIndex(null);
              }}
            >
              <svg
                viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`}
                width="100%"
                height="100%"
                preserveAspectRatio="none"
                role="img"
                aria-label="Estimated cost per day by provider"
                data-testid="daily-cost-svg"
                style={{ display: "block" }}
              >
                {value.ticks.map((tick) => {
                  const y = toY(tick);
                  return (
                    <line
                      key={tick}
                      x1={0}
                      x2={VIEW_WIDTH}
                      y1={y}
                      y2={y}
                      stroke={GRID}
                      strokeWidth={1}
                      vectorEffect="non-scaling-stroke"
                    />
                  );
                })}

                {/* Fills first, then every stroke, so no series covers another's line. */}
                {value.series.map((s) => (
                  <path key={s.slug} d={s.area} fill={s.color} fillOpacity={0.12} stroke="none" />
                ))}
                {value.series.map((s) => (
                  <path
                    key={s.slug}
                    d={s.line}
                    fill="none"
                    stroke={s.color}
                    strokeWidth={2}
                    vectorEffect="non-scaling-stroke"
                  />
                ))}

                {hoverIndex === null ? null : (
                  <line
                    x1={hoverIndex * stepX}
                    x2={hoverIndex * stepX}
                    y1={PLOT_TOP}
                    y2={VIEW_HEIGHT}
                    stroke={HOVER_LINE}
                    strokeWidth={1}
                    vectorEffect="non-scaling-stroke"
                  />
                )}
              </svg>

              {hoveredDay === undefined ? null : (
                <div
                  ref={tooltipRef}
                  style={{
                    position: "absolute",
                    zIndex: 10,
                    pointerEvents: "none",
                    minWidth: 144,
                    maxWidth: "100%",
                    left: "var(--daily-tooltip-left, 0px)",
                    top: "var(--daily-tooltip-top, 0px)",
                    background: "#ffffff",
                    border: "1px solid #e2e8f0",
                    borderRadius: 12,
                    padding: "8px 10px",
                    fontSize: 12,
                    boxShadow: "0 8px 24px rgba(15, 23, 42, 0.12)",
                  }}
                >
                  <div style={{ marginBottom: 4, color: AXIS_TEXT }}>{formatDayShort(hoveredDay).toUpperCase()}</div>
                  {value.series.map((s) => (
                    <div
                      key={s.slug}
                      style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}
                    >
                      <span style={{ display: "inline-flex", alignItems: "center", gap: 6, color: AXIS_TEXT }}>
                        <span
                          aria-hidden="true"
                          style={{ display: "inline-block", width: 8, height: 8, borderRadius: 999, background: s.color }}
                        />
                        {s.label}
                      </span>
                      <span style={{ fontVariantNumeric: "tabular-nums" }}>
                        {fmtCost(hoveredColumn?.bands.find((band) => band.slug === s.slug)?.value ?? 0)}
                      </span>
                    </div>
                  ))}
                  <div
                    style={{
                      marginTop: 4,
                      paddingTop: 4,
                      borderTop: "1px solid #e2e8f0",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "space-between",
                      gap: 12,
                    }}
                  >
                    <span style={{ color: AXIS_TEXT }}>Total</span>
                    <span style={{ fontVariantNumeric: "tabular-nums" }}>{fmtCost(hoveredColumn?.total ?? 0)}</span>
                  </div>
                </div>
              )}
            </div>
          </div>

          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              paddingLeft: 72,
              fontSize: 11,
              color: AXIS_TEXT,
              textTransform: "uppercase",
              marginTop: 4,
            }}
          >
            <span>{value.days[0] === undefined ? "" : formatDayShort(value.days[0]).toUpperCase()}</span>
            <span>
              {value.days[Math.floor(value.days.length / 2)] === undefined
                ? ""
                : formatDayShort(value.days[Math.floor(value.days.length / 2)] ?? "").toUpperCase()}
            </span>
            <span>
              {value.days[value.days.length - 1] === undefined
                ? ""
                : formatDayShort(value.days[value.days.length - 1] ?? "").toUpperCase()}
            </span>
          </div>
        </>
      ) : null}
    </section>
  );
}

function useEffectFetch(
  setValue: (next: ChartState) => void,
  since: string,
): void {
  useEffect(() => {
    const controller = new AbortController();
    setValue({ state: "loading" });
    const days = dayRange(since);
    void (async () => {
      try {
        const fetched = await Promise.all(
          PROVIDERS.map(async (p) => {
            const res = await apiGet<{
              groups?: UsageGroup[];
            }>(`/api/usage/summary?provider=${encodeURIComponent(p.slug)}&group_by=day&since=${encodeURIComponent(since)}`, {
              signal: controller.signal,
            });
            const byDay = new Map((res.groups ?? []).map((g) => [g.key, g.estimated_cost]));
            // Zero-filled like the reference: inactive days are flat zeros,
            // so every series draws one continuous line.
            return {
              slug: p.slug,
              label: p.label,
              color: PROVIDER_SERIES_COLORS[p.slug as keyof typeof PROVIDER_SERIES_COLORS] ?? "#64748b",
              hasData: (res.groups ?? []).some((g) => g.estimated_cost != null),
              costs: days.map((d) => byDay.get(d) ?? 0),
            };
          }),
        );
        if (controller.signal.aborted) return;

        const columns: DayColumn[] = days.map((_, i) => {
          const bands = fetched.map((f) => ({ slug: f.slug, value: f.costs[i] ?? 0 }));
          return { bands, total: bands.reduce((sum, band) => sum + band.value, 0) };
        });
        // The scale tops out at the largest single provider-day, not the
        // sum: layered series each measure from zero, so a combined peak
        // would leave the plot permanently half empty.
        const peak = columns.reduce(
          (max, column) => column.bands.reduce((inner, band) => Math.max(inner, band.value), max),
          0,
        );
        const { max, ticks } = niceScale(peak, TICK_COUNT);
        const step = days.length <= 1 ? 0 : VIEW_WIDTH / (days.length - 1);
        // Leave room above the top gridline so the constant-width stroke is
        // not clipped when a series reaches the peak.
        const toY = (v: number) =>
          max === 0 ? VIEW_HEIGHT : VIEW_HEIGHT - (v / max) * (VIEW_HEIGHT - PLOT_TOP);

        const built: BuiltSeries[] = fetched.map((f) => {
          const line = curvePath(
            smoothCurve(
              columns.map((column, periodIndex) => ({
                x: periodIndex * step,
                y: toY(column.bands.find((band) => band.slug === f.slug)?.value ?? 0),
              })),
            ),
          );
          return {
            slug: f.slug,
            label: f.label,
            color: f.color,
            hasData: f.hasData,
            total: columns.reduce(
              (sum, column) => sum + (column.bands.find((band) => band.slug === f.slug)?.value ?? 0),
              0,
            ),
            area: line === "" ? "" : `${line} L${VIEW_WIDTH},${VIEW_HEIGHT} L0,${VIEW_HEIGHT} Z`,
            line,
          };
        });

        // Paint the heavier series first so the lighter one is not buried.
        const ordered = [...built].sort((a, b) => b.total - a.total);
        if (!controller.signal.aborted) setValue({ state: "ready", series: ordered, days, columns, ticks, max });
      } catch (err: unknown) {
        if (controller.signal.aborted) return;
        if (err instanceof DOMException && err.name === "AbortError") return;
        setValue({ state: "error", message: err instanceof Error ? err.message : "Cost history unavailable." });
      }
    })();
    return () => {
      controller.abort();
    };
  }, [setValue, since]);
}

