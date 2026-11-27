// Live flame for the floating bar, drawn with a WebGL fragment shader.
//
// One continuous "heat" value (0 at 50% usage, 1 at 100%) drives everything:
// flame height, color temperature (blue gas flame -> blue base with orange
// tongues -> tall orange/yellow fire with a white-hot base), flicker speed
// and turbulence. Heat eases toward its target, so changes in usage roll in
// smoothly instead of jumping between stages.

const VERT = `
attribute vec2 aPos;
void main() { gl_Position = vec4(aPos, 0.0, 1.0); }
`;

const FRAG = `
precision mediump float;
uniform vec2 uRes;
uniform float uTime;
uniform float uHeat;   // 0..1
uniform float uAlpha;  // global fade in/out

float hash(vec2 p) {
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}
float noise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x),
             mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}
float fbm(vec2 p) {
  float v = 0.0;
  float a = 0.5;
  for (int i = 0; i < 5; i++) {
    v += a * noise(p);
    p = p * 2.03 + vec2(1.7, 9.2);
    a *= 0.5;
  }
  return v;
}

vec3 fireRamp(float t) {
  vec3 c = mix(vec3(0.45, 0.04, 0.0), vec3(0.95, 0.30, 0.02), smoothstep(0.0, 0.35, t));
  c = mix(c, vec3(1.0, 0.62, 0.10), smoothstep(0.30, 0.60, t));
  c = mix(c, vec3(1.0, 0.88, 0.40), smoothstep(0.55, 0.85, t));
  c = mix(c, vec3(1.0, 0.98, 0.88), smoothstep(0.85, 1.0, t));
  return c;
}
vec3 blueRamp(float t) {
  vec3 c = mix(vec3(0.03, 0.06, 0.45), vec3(0.12, 0.35, 1.0), smoothstep(0.0, 0.45, t));
  c = mix(c, vec3(0.45, 0.75, 1.0), smoothstep(0.45, 0.8, t));
  c = mix(c, vec3(0.88, 0.96, 1.0), smoothstep(0.8, 1.0, t));
  return c;
}

void main() {
  vec2 uv = gl_FragCoord.xy / uRes;          // y = 0 at the bottom
  float aspect = uRes.x / uRes.y;
  float speed = mix(1.1, 2.8, uHeat);
  float t = uTime * speed;

  // Horizontal coordinate in "flame widths": more, narrower tongues when cool.
  float x = uv.x * aspect * mix(3.2, 1.9, uHeat);

  // Local flame height varies across the bar (separate tongues) and over time.
  float maxH = mix(0.42, 1.0, uHeat);
  float tongue = maxH * (0.45 + 0.85 * fbm(vec2(x * 1.3, uTime * 0.7 * speed)));
  float y = uv.y / max(tongue, 0.05);

  // Rising turbulence bends and tears the edges.
  float n1 = fbm(vec2(x * 1.6, uv.y * 2.4 - t));
  float n2 = fbm(vec2(x * 3.4 + 3.0, uv.y * 4.0 - t * 1.5));
  float turb = mix(0.7, 1.25, uHeat);
  float f = 1.0 - y + (n1 - 0.5) * turb * y + (n2 - 0.5) * 0.35;
  f = clamp(f, 0.0, 1.0);

  float body = smoothstep(0.02, 0.30, f);
  float temp = smoothstep(0.05, 1.0, f) * (0.75 + 0.25 * n2);

  // Blue amount: orange creeps down from the tips as heat rises, leaving a
  // blue base (like a gas flame catching), and the blue is gone by the top end.
  float blueLine = mix(1.6, 0.3, smoothstep(0.08, 0.38, uHeat)) * (1.0 - smoothstep(0.55, 0.85, uHeat));
  float blue = 1.0 - smoothstep(blueLine - 0.12, blueLine + 0.12, y);

  vec3 orange = fireRamp(pow(temp, 1.35) + 0.12 * uHeat);
  vec3 gas = blueRamp(min(temp * 0.9, 0.82));
  vec3 col = mix(orange, gas, blue);
  // Blue gas flames are more translucent than orange fire.
  float a = body * mix(0.95, 0.85, blue) * uAlpha;
  gl_FragColor = vec4(col * a, a);  // premultiplied
}
`;

export interface LiveFlame {
  /** pct is 5h usage 0..100; below 50 the flame fades out. */
  setPct(pct: number): void;
  setVisible(on: boolean): void;
  destroy(): void;
}

export function createLiveFlame(canvas: HTMLCanvasElement): LiveFlame | null {
  const gl = canvas.getContext("webgl", {
    premultipliedAlpha: true,
    alpha: true,
    antialias: false,
  });
  if (!gl) return null;

  const compile = (type: number, src: string) => {
    const sh = gl.createShader(type)!;
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
      console.error("flame shader:", gl.getShaderInfoLog(sh));
      return null;
    }
    return sh;
  };
  const vs = compile(gl.VERTEX_SHADER, VERT);
  const fs = compile(gl.FRAGMENT_SHADER, FRAG);
  if (!vs || !fs) return null;
  const prog = gl.createProgram()!;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    console.error("flame program:", gl.getProgramInfoLog(prog));
    return null;
  }
  gl.useProgram(prog);

  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const aPos = gl.getAttribLocation(prog, "aPos");
  gl.enableVertexAttribArray(aPos);
  gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

  const uRes = gl.getUniformLocation(prog, "uRes");
  const uTime = gl.getUniformLocation(prog, "uTime");
  const uHeat = gl.getUniformLocation(prog, "uHeat");
  const uAlpha = gl.getUniformLocation(prog, "uAlpha");

  let targetHeat = 0;
  let heat = 0;
  let targetAlpha = 0;
  let alpha = 0;
  let visible = true;
  let raf = 0;
  let last = 0;
  let lastDraw = 0;
  const start = performance.now();
  const FRAME_MS = 1000 / 30; // 30fps is plenty for fire and saves battery

  const resize = () => {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    gl.viewport(0, 0, canvas.width, canvas.height);
  };

  const frame = (now: number) => {
    raf = 0;
    const dt = last ? Math.min(0.1, (now - last) / 1000) : 0;
    last = now;
    // Ease heat and alpha toward their targets (~1s to settle).
    heat += (targetHeat - heat) * Math.min(1, dt * 2.5);
    alpha += (targetAlpha - alpha) * Math.min(1, dt * 3.5);

    if (now - lastDraw >= FRAME_MS - 1) {
      lastDraw = now;
      resize();
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.uniform2f(uRes, canvas.width, canvas.height);
      gl.uniform1f(uTime, (now - start) / 1000);
      gl.uniform1f(uHeat, heat);
      gl.uniform1f(uAlpha, alpha);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    }

    // Stop the loop entirely once faded out.
    if (targetAlpha === 0 && alpha < 0.005) {
      alpha = 0;
      gl.clear(gl.COLOR_BUFFER_BIT);
      last = 0;
      return;
    }
    raf = requestAnimationFrame(frame);
  };
  const kick = () => {
    if (!raf) raf = requestAnimationFrame(frame);
  };

  const update = (pct: number) => {
    const on = visible && pct >= 50;
    if (on && alpha === 0) heat = Math.min(1, Math.max(0, (pct - 50) / 50));
    targetHeat = Math.min(1, Math.max(0, (pct - 50) / 50));
    targetAlpha = on ? 1 : 0;
    if (on || alpha > 0) kick();
  };
  let lastPct = 0;

  return {
    setPct(pct: number) {
      lastPct = pct;
      update(pct);
    },
    setVisible(on: boolean) {
      visible = on;
      update(lastPct);
    },
    destroy() {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
    },
  };
}
