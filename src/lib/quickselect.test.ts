import { describe, expect, it } from "vitest";
import { findMatches, labelMatches, makeLabels, type ScreenLine } from "./quickselect";

/** A logical line whose characters sit one per column on one row. */
const line = (text: string, row = 0): ScreenLine =>
  ({ text, cells: [...text].map((_, column) => ({ row, column })) });
const texts = (source: string) => findMatches([line(source)]).map(match => match.text);

describe("finding what is worth copying", () => {
  it("finds container ids and image tags on a docker ps line", () => {
    expect(texts("a91c04e6f3b2   postgres:16    Up 2 hours")).toEqual(["a91c04e6f3b2"]);
  });

  it("finds addresses, with a port or prefix when there is one", () => {
    expect(texts("inet 10.40.12.9/24 brd 10.40.12.255 scope global")).toEqual(["10.40.12.9/24", "10.40.12.255"]);
    expect(texts("listening on 127.0.0.1:8080")).toEqual(["127.0.0.1:8080"]);
    expect(texts("inet6 fe80::1c2b:3aff:fe4d:5e6f/64")).toEqual(["fe80::1c2b:3aff:fe4d:5e6f"]);
  });

  it("does not take a time of day for an IPv6 address", () => {
    expect(texts("since 08:12:44 UTC")).toEqual([]);
  });

  it("keeps a UUID whole rather than as runs of hex", () => {
    expect(texts("id=3f2a9c81-b7de-4c1a-9e2f-0a1b2c3d4e5f ok")).toEqual(["3f2a9c81-b7de-4c1a-9e2f-0a1b2c3d4e5f"]);
  });

  it("finds a short git hash but not a word spelt from a to f", () => {
    expect(texts("4d3fd74 Check the installer, decade facade")).toEqual(["4d3fd74"]);
  });

  it("finds paths that start a word, and not the slash in and/or", () => {
    expect(texts("wrote /var/log/nginx/error.log and ~/notes.md, see ./build/out")).toEqual(
      ["/var/log/nginx/error.log", "~/notes.md", "./build/out"]);
    expect(texts("read and/or write")).toEqual([]);
  });

  it("trims the sentence off a URL but keeps its own brackets", () => {
    expect(texts("see https://example.com/docs.")).toEqual(["https://example.com/docs"]);
    expect(texts("(https://en.wikipedia.org/wiki/Foo_(bar))")).toEqual(["https://en.wikipedia.org/wiki/Foo_(bar)"]);
  });

  it("finds process ids and other long numbers, and not short ones", () => {
    expect(texts("nginx 23817 0.0 1.2 9 12")).toEqual(["23817"]);
  });

  it("takes each piece of text once, most specific pattern first", () => {
    // Not also the number 8080, nor the address without its port.
    expect(findMatches([line("http://10.0.0.5:8080/health")]).map(match => match.text))
      .toEqual(["http://10.0.0.5:8080/health"]);
  });

  it("orders matches down the screen and across it", () => {
    const found = findMatches([line("/b 10.0.0.2", 0), line("/a", 1)]);
    expect(found.map(match => [match.line, match.text])).toEqual([[0, "/b"], [0, "10.0.0.2"], [1, "/a"]]);
  });
});

describe("labelling", () => {
  it("uses one letter each while they last, starting on the home row", () => {
    expect(makeLabels(3)).toEqual(["a", "s", "d"]);
  });

  /// A mix of one- and two-letter labels would make the first letter of a
  /// two-letter label ambiguous with a one-letter one.
  it("switches every label to two letters past the alphabet", () => {
    const labels = makeLabels(30);
    expect(labels).toHaveLength(30);
    expect(labels.every(label => label.length === 2)).toBe(true);
    expect(new Set(labels).size).toBe(30);
  });

  it("gives the same text the same label", () => {
    const labelled = labelMatches(findMatches([line("a91c04e6f3b2 and a91c04e6f3b2 and 10.0.0.1")]));
    expect(labelled.map(match => match.label)).toEqual(["a", "a", "s"]);
  });
});
