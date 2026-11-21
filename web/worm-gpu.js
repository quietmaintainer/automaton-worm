// The detailed geometry from Automaton Swarm, drawn into a transparent layer.
function rgb(hex) {
  const value = Number.parseInt(hex.slice(1), 16);
  return `vec3(${value >>> 16}.0, ${(value >>> 8) & 255}.0, ${value & 255}.0) / 255.0`;
}

function compileProgram(gl, vertex, fragment) {
  const program = gl.createProgram();
  for (const [type, source] of [[gl.VERTEX_SHADER, vertex], [gl.FRAGMENT_SHADER, fragment]]) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(shader));
    gl.attachShader(program, shader);
    gl.deleteShader(shader);
  }
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program));
  return program;
}

function integerTexture(gl, slot, width, format, type, ArrayType) {
  gl.activeTexture(gl.TEXTURE0 + slot);
  gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  let previous;
  let pixels;
  return values => {
    if (values === previous) return;
    const height = Math.max(1, Math.ceil(values.length / width));
    gl.activeTexture(gl.TEXTURE0 + slot);
    if (pixels?.length !== width * height) {
      pixels = new ArrayType(width * height);
      gl.texImage2D(gl.TEXTURE_2D, 0, format, width, height, 0, gl.RED_INTEGER, type, null);
    }
    pixels.set(values);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, width, height, gl.RED_INTEGER, type, pixels);
    previous = values;
  };
}

export function createWormGpu(style) {
  const canvas = document.createElement('canvas');
  const gl = canvas.getContext('webgl2', {
    antialias: false, depth: false, stencil: false, failIfMajorPerformanceCaveat: true,
  });
  if (!gl) return null;
  const width = Math.min(2048, gl.getParameter(gl.MAX_TEXTURE_SIZE));
  const { length, colors, background, food, linkRadius, bodyRadius, headRadius, centerRadius, eyeRadius, eyeOffset, pelletRadius } = style;

  const program = compileProgram(gl, `#version 300 es
    precision highp float;
    precision highp int;
    uniform highp usampler2D bodies, headings, pellets;
    uniform vec2 resolution, origin;
    uniform float unit;
    uniform uint edge;
    uniform int kind;
    flat out int colorIndex, head;
    flat out vec2 start, end, eye;
    out vec2 position;
    const vec2 corners[6] = vec2[6](vec2(0,0), vec2(1,0), vec2(0,1), vec2(0,1), vec2(1,0), vec2(1,1));
    const vec2 directions[4] = vec2[4](vec2(-1,0), vec2(1,0), vec2(0,-1), vec2(0,1));
    uint readSite(highp usampler2D sites, int n) {
      return texelFetch(sites, ivec2(n % ${width}, n / ${width}), 0).r;
    }
    vec2 cellCenter(uint site) { return vec2(site % edge, site / edge) + 0.5; }
    void main() {
      int n = gl_InstanceID;
      head = 0;
      eye = vec2(0);
      if (kind == 0) {
        start = cellCenter(readSite(pellets, n));
        end = start;
        colorIndex = 0;
      } else if (kind == 1) {
        int worm = n / ${length - 1};
        int segment = worm * ${length} + n % ${length - 1};
        start = cellCenter(readSite(bodies, segment));
        end = cellCenter(readSite(bodies, segment + 1));
        colorIndex = worm % ${colors.length};
      } else {
        int worm = n / ${length};
        start = cellCenter(readSite(bodies, n));
        end = start;
        colorIndex = worm % ${colors.length};
        head = int(n % ${length} == 0);
        if (head == 1) eye = directions[int(readSite(headings, worm) & 3u)] * ${eyeOffset};
      }
      float pad = (kind == 1 ? ${linkRadius} : ${headRadius}) + 1.0 / unit;
      position = mix(min(start, end) - pad, max(start, end) + pad, corners[gl_VertexID]);
      vec2 clip = (origin + position * unit) / resolution;
      gl_Position = vec4(2.0 * clip.x - 1.0, 1.0 - 2.0 * clip.y, 0.0, 1.0);
    }`, `#version 300 es
    precision highp float;
    precision highp int;
    uniform float unit;
    uniform int kind;
    flat in int colorIndex, head;
    flat in vec2 start, end, eye;
    in vec2 position;
    out vec4 color;
    const vec3 palette[${colors.length}] = vec3[${colors.length}](${colors.map(rgb).join(', ')});
    float coverage(float radius, float distanceFromCenter) {
      return clamp(0.5 + (radius - distanceFromCenter) * unit, 0.0, 1.0);
    }
    void main() {
      vec3 rgb = palette[colorIndex];
      float alpha;
      if (kind == 0) {
        vec2 delta = abs(position - start);
        alpha = coverage(${pelletRadius}, delta.x + delta.y);
        rgb = ${rgb(food)};
      } else if (kind == 1) {
        vec2 segment = end - start;
        float t = clamp(dot(position - start, segment) / max(0.001, dot(segment, segment)), 0.0, 1.0);
        alpha = coverage(${linkRadius}, length(position - start - t * segment));
      } else {
        alpha = coverage(head == 1 ? ${headRadius} : ${bodyRadius}, length(position - start));
        rgb = mix(rgb, ${rgb(background)}, coverage(head == 1 ? ${eyeRadius} : ${centerRadius}, length(position - start - eye)));
      }
      color = vec4(rgb, alpha);
    }`);

  gl.useProgram(program);
  const uniforms = Object.fromEntries(['bodies', 'headings', 'pellets', 'resolution', 'origin', 'unit', 'edge', 'kind']
    .map(name => [name, gl.getUniformLocation(program, name)]));
  gl.uniform1i(uniforms.bodies, 0);
  gl.uniform1i(uniforms.headings, 1);
  gl.uniform1i(uniforms.pellets, 2);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  gl.clearColor(0, 0, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  const uploadBodies = integerTexture(gl, 0, width, gl.R32UI, gl.UNSIGNED_INT, Uint32Array);
  const uploadHeadings = integerTexture(gl, 1, width, gl.R8UI, gl.UNSIGNED_BYTE, Uint8Array);
  const uploadPellets = integerTexture(gl, 2, width, gl.R32UI, gl.UNSIGNED_INT, Uint32Array);

  return ({ bodies, headings, pellets }, size, surfaceWidth, surfaceHeight, { unit, x, y }) => {
    if (gl.isContextLost()) return null;
    if (canvas.width !== surfaceWidth) canvas.width = surfaceWidth;
    if (canvas.height !== surfaceHeight) canvas.height = surfaceHeight;
    uploadBodies(bodies);
    uploadHeadings(headings);
    uploadPellets(pellets);
    gl.viewport(0, 0, surfaceWidth, surfaceHeight);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.uniform2f(uniforms.resolution, surfaceWidth, surfaceHeight);
    gl.uniform2f(uniforms.origin, x, y);
    gl.uniform1f(uniforms.unit, unit);
    gl.uniform1ui(uniforms.edge, size);
    const counts = [pellets.length, headings.length * (length - 1), bodies.length];
    for (const [kind, count] of counts.entries()) {
      gl.uniform1i(uniforms.kind, kind);
      gl.drawArraysInstanced(gl.TRIANGLES, 0, 6, count);
    }
    return canvas;
  };
}
