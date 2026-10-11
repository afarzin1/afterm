import type { SessionProfile } from "../types";

export function isSafeSshIdentity(value: string): boolean {
  return value.length > 0 && !value.startsWith("-") && !/\s/.test(value);
}

/**
 * What a host is called: the name someone gave it, or the address it lives at.
 *
 * A blank name is not a mistake to reject, it is the absence of one — clearing
 * the field puts a host back to being known by its address rather than leaving
 * a nameless row nobody can pick out of the list. Only when there is no address
 * either is a placeholder invented, which is the local-shell case.
 */
export function profileName(given: string, host?: string, fallback = "Saved host"): string {
  return given.trim() || host?.trim() || fallback;
}

/** Rename one saved host, leaving the rest of the list as it was. */
export function renameProfile(
  profiles: SessionProfile[],
  id: string,
  given: string
): SessionProfile[] {
  return profiles.map(profile => profile.id === id
    ? { ...profile, name: profileName(given, profile.host) }
    : profile);
}
