/**
 * Tests for scripts/hooks/post-edit-accumulator.js and
 *           scripts/hooks/stop-format-typecheck.js
 *
 * Run with: node tests/hooks/stop-format-typecheck.test.js
 */

'use strict';

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const accumulator = require('../../scripts/hooks/post-edit-accumulator');

function test(name, fn) {
  try {
    fn();
    console.log(`  ✓ ${name}`);
    return true;
  } catch (err) {
    console.log(`  ✗ ${name}`);
    console.log(`    Error: ${err.message}`);
    return false;
  }
}

let passed = 0;
let failed = 0;

// Use a unique session ID for tests so we don't pollute real sessions
const TEST_SESSION_ID = `test-${Date.now()}`;
const origSessionId = process.env.CLAUDE_SESSION_ID;
process.env.CLAUDE_SESSION_ID = TEST_SESSION_ID;

function getAccumFile() {
  return path.join(os.tmpdir(), `ecc-edited-${TEST_SESSION_ID}.txt`);
}

function cleanAccumFile() {
  try { fs.unlinkSync(getAccumFile()); } catch { /* doesn't exist */ }
}

// ── post-edit-accumulator.js ─────────────────────────────────────

console.log('\npost-edit-accumulator: pass-through behavior');
console.log('=============================================\n');

if (test('returns original input unchanged', () => {
  cleanAccumFile();
  const input = JSON.stringify({ tool_input: { file_path: '/tmp/x.ts' } });
  const result = accumulator.run(input);
  assert.strictEqual(result, input);
  cleanAccumFile();
})) passed++; else failed++;

if (test('returns original input for invalid JSON', () => {
  cleanAccumFile();
  const input = 'not json';
  const result = accumulator.run(input);
  assert.strictEqual(result, input);
})) passed++; else failed++;

if (test('returns original input when no file_path', () => {
  cleanAccumFile();
  const input = JSON.stringify({ tool_input: { command: 'ls' } });
  const result = accumulator.run(input);
  assert.strictEqual(result, input);
  cleanAccumFile();
})) passed++; else failed++;

console.log('\npost-edit-accumulator: file accumulation');
console.log('=========================================\n');

if (test('creates accumulator file for a .ts file', () => {
  cleanAccumFile();
  const input = JSON.stringify({ tool_input: { file_path: '/tmp/foo.ts' } });
  accumulator.run(input);
  const accumFile = getAccumFile();
  assert.ok(fs.existsSync(accumFile), 'accumulator file should exist');
  const lines = fs.readFileSync(accumFile, 'utf8').split('\n').filter(Boolean);
  assert.ok(lines.includes('/tmp/foo.ts'));
  cleanAccumFile();
})) passed++; else failed++;

if (test('accumulates multiple files across calls', () => {
  cleanAccumFile();
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/a.ts' } }));
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/b.tsx' } }));
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/c.js' } }));
  const lines = fs.readFileSync(getAccumFile(), 'utf8').split('\n').filter(Boolean);
  assert.deepStrictEqual(lines, ['/tmp/a.ts', '/tmp/b.tsx', '/tmp/c.js']);
  cleanAccumFile();
})) passed++; else failed++;

if (test('concurrent writes are preserved (no data lost via append)', () => {
  cleanAccumFile();
  // Simulate concurrent writes — each appends independently
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/a.ts' } }));
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/b.ts' } }));
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/a.ts' } })); // duplicate
  const lines = fs.readFileSync(getAccumFile(), 'utf8').split('\n').filter(Boolean);
  // All three appends land; dedup happens at read time in the Stop hook
  assert.strictEqual(lines.length, 3);
  const unique = [...new Set(lines)];
  assert.strictEqual(unique.length, 2);
  cleanAccumFile();
})) passed++; else failed++;

if (test('does not create accumulator file for non-JS/TS files', () => {
  cleanAccumFile();
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/README.md' } }));
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/styles.css' } }));
  assert.ok(!fs.existsSync(getAccumFile()), 'no accumulator for non-JS/TS files');
})) passed++; else failed++;

if (test('handles .tsx and .jsx extensions', () => {
  cleanAccumFile();
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/comp.tsx' } }));
  accumulator.run(JSON.stringify({ tool_input: { file_path: '/tmp/comp.jsx' } }));
  const lines = fs.readFileSync(getAccumFile(), 'utf8').split('\n').filter(Boolean);
  assert.ok(lines.includes('/tmp/comp.tsx'));
  assert.ok(lines.includes('/tmp/comp.jsx'));
  cleanAccumFile();
})) passed++; else failed++;

// ── stop-format-typecheck: accumulator teardown ──────────────────

console.log('\nstop-format-typecheck: accumulator cleanup');
console.log('==========================================\n');

if (test('stop hook removes accumulator file after reading it', () => {
  cleanAccumFile();
  // Write a fake accumulator with a non-existent file so no real formatter runs
  fs.writeFileSync(getAccumFile(), '/nonexistent/file.ts\n', 'utf8');
  assert.ok(fs.existsSync(getAccumFile()), 'accumulator should exist before stop hook');

  // Require the stop hook and invoke main() directly via its stdin entry.
  // We simulate the stdin+stdout flow by spawning node and feeding empty stdin.
  const { execFileSync } = require('child_process');
  const stopScript = path.resolve(__dirname, '../../scripts/hooks/stop-format-typecheck.js');
  try {
    execFileSync('node', [stopScript], {
      input: '{}',
      env: { ...process.env, CLAUDE_SESSION_ID: TEST_SESSION_ID },
      stdio: ['pipe', 'pipe', 'pipe'],
      timeout: 10000
    });
  } catch {
    // tsc/formatter may fail for the nonexistent file — that's OK
  }

  assert.ok(!fs.existsSync(getAccumFile()), 'accumulator file should be deleted by stop hook');
})) passed++; else failed++;

if (test('stop hook is a no-op when no accumulator exists', () => {
  cleanAccumFile();
  const { execFileSync } = require('child_process');
  const stopScript = path.resolve(__dirname, '../../scripts/hooks/stop-format-typecheck.js');
  // Should exit cleanly with no errors
  execFileSync('node', [stopScript], {
    input: '{}',
    env: { ...process.env, CLAUDE_SESSION_ID: TEST_SESSION_ID },
    stdio: ['pipe', 'pipe', 'pipe'],
    timeout: 10000
  });
})) passed++; else failed++;

if (test('stop hook passes stdin through unchanged', () => {
  cleanAccumFile();
  const { execFileSync } = require('child_process');
  const stopScript = path.resolve(__dirname, '../../scripts/hooks/stop-format-typecheck.js');
  const input = '{"stop_reason":"end_turn"}';
  const result = execFileSync('node', [stopScript], {
    input,
    env: { ...process.env, CLAUDE_SESSION_ID: TEST_SESSION_ID },
    stdio: ['pipe', 'pipe', 'pipe'],
    timeout: 10000
  });
  assert.strictEqual(result.toString(), input);
})) passed++; else failed++;

// Restore env
if (origSessionId === undefined) {
  delete process.env.CLAUDE_SESSION_ID;
} else {
  process.env.CLAUDE_SESSION_ID = origSessionId;
}

console.log(`\n=== Test Results ===`);
console.log(`Passed: ${passed}`);
console.log(`Failed: ${failed}`);
console.log(`Total:  ${passed + failed}`);

process.exit(failed > 0 ? 1 : 0);
