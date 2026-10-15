// The WebGL path of <signal-canvas>: one small program that draws world
// points (x, y, size) as round point sprites or as line pairs, mapped by
// the same fit as Canvas 2D (ui/lib/canvas.js). Used when the browser gives
// a WebGL context and the renderer is unskinned points or mesh.

const VERTEX = `
attribute vec3 a;
uniform vec3 fit;   // scale, x0, y0 in device pixels
uniform vec2 size;  // the canvas in device pixels
void main() {
  vec2 px = vec2(fit.y + fit.x * a.x, fit.z - fit.x * a.y);
  gl_Position = vec4(px.x / size.x * 2.0 - 1.0, 1.0 - px.y / size.y * 2.0, 0.0, 1.0);
  gl_PointSize = a.z;
}`;

const FRAGMENT = `
precision mediump float;
uniform vec4 colour;
uniform bool round;
void main() {
  if (round && length(gl_PointCoord - vec2(0.5)) > 0.5) discard;
  gl_FragColor = colour;
}`;

function shader(gl, type, source) {
  const s = gl.createShader(type);
  gl.shaderSource(s, source);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader');
  return s;
}

/** A WebGL drawer for `canvas`, or null when there is no WebGL. */
export function webglDrawer(canvas) {
  const gl = canvas.getContext('webgl', { premultipliedAlpha: false, antialias: true });
  if (!gl) return null;
  const program = gl.createProgram();
  gl.attachShader(program, shader(gl, gl.VERTEX_SHADER, VERTEX));
  gl.attachShader(program, shader(gl, gl.FRAGMENT_SHADER, FRAGMENT));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return null;
  const buffer = gl.createBuffer();
  const where = (name) => gl.getUniformLocation(program, name);
  return {
    /** Draws `{ mode, data }` (x, y, size triples) in `rgba` (0..1). */
    draw({ mode, data }, fit, rgba) {
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      if (data.length === 0) return;
      gl.useProgram(program);
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.bufferData(gl.ARRAY_BUFFER, data, gl.STREAM_DRAW);
      const a = gl.getAttribLocation(program, 'a');
      gl.enableVertexAttribArray(a);
      gl.vertexAttribPointer(a, 3, gl.FLOAT, false, 0, 0);
      gl.uniform3f(where('fit'), fit.scale, fit.x0, fit.y0);
      gl.uniform2f(where('size'), canvas.width, canvas.height);
      gl.uniform4f(where('colour'), ...rgba);
      gl.uniform1i(where('round'), mode === 'points' ? 1 : 0);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      gl.drawArrays(mode === 'points' ? gl.POINTS : gl.LINES, 0, data.length / 3);
    },
  };
}
