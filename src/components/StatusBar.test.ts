import { describe, expect, it } from "vitest";
import { authenticationOf } from "./StatusBar";

describe("status-bar authentication labels", () => {
  it("shows the authentication kind without a public-key path", () => {
    expect(authenticationOf("native · publickey (/home/me/.ssh/id_ed25519)"))
      .toBe("public key");
    expect(authenticationOf("native · agent (work laptop)"))
      .toBe("public key");
  });

  it("keeps password auth and leaves fallback transports separate", () => {
    expect(authenticationOf("native · password")).toBe("password");
    expect(authenticationOf("OpenSSH client")).toBeNull();
  });
});
