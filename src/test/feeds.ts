import type { Feed, FeedItem } from "../lib/ipc";

// Feeds (Labs) fixtures for the mock harness and tests: one blog, one podcast,
// one feed whose last refresh failed. Content mirrors what the Rust sanitizer
// emits (no images, links rel=noopener).
const DAY = 86_400_000;
const NOW = Date.UTC(2026, 10, 25, 15, 0, 0);

export const demoFeeds: Feed[] = [
  {
    id: "f-blog",
    url: "https://www.databricks.com/feed",
    title: "Databricks Blog",
    site_url: "https://www.databricks.com/blog",
    kind: "rss",
    image_url: "",
    last_fetched_at: NOW,
    error: "",
    unread: 2,
  },
  {
    id: "f-pod",
    url: "https://feeds.example.com/data-brew",
    title: "Data Brew",
    site_url: "https://example.com/data-brew",
    kind: "podcast",
    image_url: "",
    last_fetched_at: NOW,
    error: "",
    unread: 1,
  },
  {
    id: "f-dead",
    url: "https://gone.example.org/rss",
    title: "Old Newsletter",
    site_url: "",
    kind: "rss",
    image_url: "",
    last_fetched_at: NOW - DAY,
    error: "https://gone.example.org/rss answered 404 Not Found",
    unread: 0,
  },
];

const item = (p: Partial<FeedItem> & Pick<FeedItem, "id" | "feed_id" | "title">): FeedItem => ({
  guid: p.id,
  link: `https://example.com/${p.id}`,
  author: "",
  published_at: NOW,
  content_html: "",
  enclosure_url: "",
  enclosure_type: "",
  enclosure_bytes: 0,
  duration_s: 0,
  read: false,
  note_id: null,
  summary: "",
  ...p,
});

export const demoFeedItems: FeedItem[] = [
  item({
    id: "i1",
    feed_id: "f-blog",
    title: "Data Modeling Best Practices for an Agentic CDP, Part 1: The Data Foundation",
    author: "Databricks",
    published_at: NOW - 2 * 3_600_000,
    content_html:
      '<p>Customer data platforms were built for dashboards. Agents need something else: a governed, ' +
      "queryable foundation they can reason over without a human in the loop.</p>" +
      "<h2>Start from identity</h2><p>Resolve people and accounts once, in Unity Catalog, and let every " +
      'downstream table join on that key. <a href="https://docs.databricks.com/" rel="noopener noreferrer">The docs</a> ' +
      "cover the identity graph pattern in depth.</p><ul><li>One golden profile per person</li>" +
      "<li>Event tables append-only, partitioned by day</li><li>Metric views for anything an agent will ask twice</li></ul>" +
      "<blockquote>Model for the question, not the source system.</blockquote>",
  }),
  item({
    id: "i2",
    feed_id: "f-pod",
    title: "Episode 42: Lakebase in production with the Acme platform team",
    published_at: NOW - DAY,
    enclosure_url: "https://cdn.example.com/data-brew/42.mp3",
    enclosure_type: "audio/mpeg",
    enclosure_bytes: 48_000_000,
    duration_s: 3_420,
    content_html: "<p>How a platform team moved its operational store onto Lakebase, and what broke first.</p>",
  }),
  item({
    id: "i3",
    feed_id: "f-blog",
    title: "Announcing serverless GPU compute for notebooks",
    summary:
      "Databricks notebooks can now attach GPUs on demand, with no cluster to configure.\n\n" +
      "- Per-second billing; GPUs release when the notebook goes idle\n" +
      "- A10G and H100 available at launch, more regions to follow\n" +
      "- Works with existing ML runtimes and Unity Catalog permissions\n\n" +
      "**Why it matters:** it removes the cluster-sizing step that slows ad hoc GPU work.",
    published_at: NOW - 3 * DAY,
    content_html: "<p>GPUs on demand, billed by the second, with no cluster to size.</p>",
  }),
  item({
    id: "i4",
    feed_id: "f-pod",
    title: "Episode 41: Evaluating agents without vibes",
    published_at: NOW - 8 * DAY,
    enclosure_url: "https://cdn.example.com/data-brew/41.mp3",
    enclosure_type: "audio/mpeg",
    duration_s: 2_710,
    read: true,
    note_id: "n-ep41",
  }),
  item({
    id: "i5",
    feed_id: "f-blog",
    title: "Unity Catalog metric views are generally available",
    published_at: NOW - 12 * DAY,
    read: true,
  }),
];
