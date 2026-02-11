// Generate a realistic large JS module with classes, closures, validators, etc.
// Usage: node generate_large_module.js <size_kb> > output.js

const sizeKB = parseInt(process.argv[2] || '500');
const targetBytes = sizeKB * 1024;

let output = '';

// Helper to generate realistic-looking code blocks
function addClass(n) {
  return `
class Service${n} {
  constructor(config) {
    this.name = "service_${n}";
    this.config = config || {};
    this.cache = new Map();
    this.listeners = [];
  }
  
  async process(input) {
    const key = JSON.stringify(input);
    if (this.cache.has(key)) return this.cache.get(key);
    const result = await this._transform(input);
    this.cache.set(key, result);
    return result;
  }
  
  _transform(data) {
    return Object.entries(data).reduce((acc, [k, v]) => {
      acc[k.toUpperCase()] = typeof v === 'string' ? v.trim() : v;
      return acc;
    }, {});
  }
  
  subscribe(fn) {
    this.listeners.push(fn);
    return () => { this.listeners = this.listeners.filter(l => l !== fn); };
  }
  
  notify(event) {
    for (const fn of this.listeners) fn(event);
  }
}
`;
}

function addValidator(n) {
  return `
const validator${n} = {
  isString: (v) => typeof v === 'string',
  isNumber: (v) => typeof v === 'number' && !isNaN(v),
  isEmail: (v) => typeof v === 'string' && /^[^@]+@[^@]+\\.[^@]+$/.test(v),
  isUrl: (v) => { try { new URL(v); return true; } catch { return false; } },
  minLength: (min) => (v) => typeof v === 'string' && v.length >= min,
  maxLength: (max) => (v) => typeof v === 'string' && v.length <= max,
  inRange: (min, max) => (v) => typeof v === 'number' && v >= min && v <= max,
  matches: (pattern) => (v) => pattern.test(v),
  oneOf: (...values) => (v) => values.includes(v),
  shape: (schema) => (obj) => {
    if (typeof obj !== 'object' || obj === null) return false;
    for (const [key, validate] of Object.entries(schema)) {
      if (!validate(obj[key])) return false;
    }
    return true;
  },
};
`;
}

function addUtility(n) {
  return `
function deepMerge${n}(target, ...sources) {
  for (const source of sources) {
    for (const key of Object.keys(source)) {
      if (source[key] instanceof Object && key in target) {
        Object.assign(source[key], deepMerge${n}(target[key], source[key]));
      }
    }
    Object.assign(target || {}, source);
  }
  return target;
}

function debounce${n}(fn, delay) {
  let timer;
  return function(...args) {
    clearTimeout(timer);
    timer = setTimeout(() => fn.apply(this, args), delay);
  };
}

function memoize${n}(fn) {
  const cache = new Map();
  return function(...args) {
    const key = JSON.stringify(args);
    if (cache.has(key)) return cache.get(key);
    const result = fn.apply(this, args);
    cache.set(key, result);
    return result;
  };
}

const constants${n} = Object.freeze({
  MAX_RETRIES: ${3 + n % 10},
  TIMEOUT_MS: ${1000 + n * 100},
  BATCH_SIZE: ${50 + n * 10},
  API_VERSION: "v${1 + n % 5}",
  ENDPOINTS: [
    "/api/users", "/api/posts", "/api/comments",
    "/api/auth", "/api/files", "/api/search"
  ],
  STATUS_CODES: { OK: 200, CREATED: 201, BAD_REQUEST: 400, NOT_FOUND: 404, ERROR: 500 },
  HEADERS: { "Content-Type": "application/json", "Accept": "application/json" },
});
`;
}

// Build the module
output += '// Auto-generated large module for benchmarking\n';
output += '"use strict";\n\n';

let n = 0;
while (output.length < targetBytes) {
  output += addClass(n);
  output += addValidator(n);
  output += addUtility(n);
  n++;
}

// Add exports
output += '\nexport {\n';
for (let i = 0; i < n; i++) {
  output += `  Service${i},\n`;
  output += `  validator${i},\n`;
  output += `  deepMerge${i},\n`;
}
output += '};\n';

process.stdout.write(output);
