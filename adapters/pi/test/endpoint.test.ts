import assert from "node:assert/strict";
import test from "node:test";
import { endpointName, pipePath, resolveEndpoint, unixSocketPath, userTag } from "../src/endpoint.ts";

test("endpoint name defaults and validates KEPLER_IPC_NAME", () => {
  assert.equal(endpointName({}), "daemon");
  assert.equal(endpointName({ KEPLER_IPC_NAME: "test-1_a" }), "test-1_a");
  for (const bad of ["", "A", "a/b", "..", "a b", "x".repeat(33), "a\\b"]) {
    assert.equal(endpointName({ KEPLER_IPC_NAME: bad }), "daemon", bad);
  }
});

test("user tag is sanitized, falls back and is capped", () => {
  assert.equal(userTag({ USERNAME: "Wil Reyes" }), "Wil_Reyes");
  assert.equal(userTag({ USERNAME: "", USER: "bob" }), "bob");
  assert.equal(userTag({}), "user");
  assert.equal(userTag({ USERNAME: "a".repeat(40) }).length, 32);
  assert.equal(userTag({ USERNAME: "DOM\\wil" }), "DOM_wil");
});

test("windows pipe path", () => {
  assert.equal(pipePath({ USERNAME: "wil", KEPLER_IPC_NAME: "t1" }), "\\\\.\\pipe\\kepler-wil-t1");
  assert.equal(resolveEndpoint({ env: { USERNAME: "wil" }, platform: "win32" }), "\\\\.\\pipe\\kepler-wil-daemon");
});

test("unix socket path prefers XDG_RUNTIME_DIR, else tmp/kepler-<user>", () => {
  const norm = (p: string) => p.replaceAll("\\", "/");
  assert.equal(norm(unixSocketPath({ XDG_RUNTIME_DIR: "/run/user/1000" }, "/tmp")), "/run/user/1000/kepler/kepler-daemon.sock");
  assert.equal(norm(unixSocketPath({ USER: "bob", KEPLER_IPC_NAME: "x" }, "/tmp")), "/tmp/kepler-bob/kepler-x.sock");
  assert.equal(norm(unixSocketPath({ XDG_RUNTIME_DIR: "", USER: "bob" }, "/tmp")), "/tmp/kepler-bob/kepler-daemon.sock");
  assert.equal(norm(resolveEndpoint({ env: { USER: "bob" }, platform: "linux", tmp: "/tmp" })), "/tmp/kepler-bob/kepler-daemon.sock");
});
