import { describe, expect, it } from "vitest";
import { isSafeSshIdentity, profileName, renameProfile } from "./profile";
import type { SessionProfile } from "../types";

describe("SSH identity validation", () => {
  it("accepts ordinary hostnames and usernames", () => {
    expect(isSafeSshIdentity("deploy@example.internal")).toBe(true);
  });

  it("rejects whitespace and option injection", () => {
    expect(isSafeSshIdentity("host name")).toBe(false);
    expect(isSafeSshIdentity("-oProxyCommand=evil")).toBe(false);
  });
});

describe("naming a saved host", () => {
  it("prefers the name someone gave it", () => {
    expect(profileName("  Production web  ", "10.0.0.4")).toBe("Production web");
  });

  it("falls back to the address when the name is cleared", () => {
    expect(profileName("   ", "10.0.0.4")).toBe("10.0.0.4");
  });

  it("still has something to show for a host with no address", () => {
    expect(profileName("", undefined)).toBe("Saved host");
  });

  it("renames one host and leaves the others untouched", () => {
    const profiles: SessionProfile[] = [
      { id: "a", name: "10.0.0.4", kind: "ssh", host: "10.0.0.4", user: "deploy", port: 22 },
      { id: "b", name: "10.0.0.5", kind: "ssh", host: "10.0.0.5", user: "deploy", port: 22 }
    ];
    const renamed = renameProfile(profiles, "a", "Production web");
    expect(renamed.map(item => item.name)).toEqual(["Production web", "10.0.0.5"]);
    // The address itself is untouched: a name is a label, not a destination.
    expect(renamed[0].host).toBe("10.0.0.4");
    expect(renamed[1]).toBe(profiles[1]);
  });

  it("puts a host back to its address when the name is emptied", () => {
    const profiles: SessionProfile[] = [
      { id: "a", name: "Production web", kind: "ssh", host: "10.0.0.4", user: "deploy", port: 22 }
    ];
    expect(renameProfile(profiles, "a", "  ")[0].name).toBe("10.0.0.4");
  });
});
