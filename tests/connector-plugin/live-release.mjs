// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// The release a live run expects is the one this checkout builds, so a version bump needs no script edit.
export function releaseVersion(cargoToml) {
  const package_ = cargoToml.split(/^\[/m).find(section => section.startsWith('package]'));
  const version = package_?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version || !/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Cargo.toml has no MAJOR.MINOR.PATCH package version');
  return version;
}

// A Jira probe that cannot pick exactly one authorized site has tested nothing, so it must count as incomplete.
export function jiraSite(status, sites) {
  if (status < 200 || status >= 300) return { blocked: `accessible resources HTTP ${status}` };
  const unique = [...new Map((sites ?? []).filter(site => site.scopes?.includes('read:jira-work')).map(site => [site.id, site])).values()];
  if (unique.length !== 1) return { blocked: `select one of ${unique.length} accessible Jira sites before tool testing` };
  return { site: unique[0] };
}
