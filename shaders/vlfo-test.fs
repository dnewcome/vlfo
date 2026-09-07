/*{
  "DESCRIPTION": "vlfo first-slice test: rotating bars. speed and hue are meant to be driven from Pd over OSC.",
  "CREDIT": "vlfo",
  "CATEGORIES": ["Generator"],
  "INPUTS": [
    {"NAME": "speed",  "TYPE": "float",   "DEFAULT": 0.5, "MIN": 0.0, "MAX": 4.0},
    {"NAME": "hue",    "TYPE": "float",   "DEFAULT": 0.0, "MIN": 0.0, "MAX": 1.0},
    {"NAME": "bars",   "TYPE": "long",    "DEFAULT": 8,   "MIN": 1,   "MAX": 64},
    {"NAME": "invert", "TYPE": "bool",    "DEFAULT": false},
    {"NAME": "tint",   "TYPE": "color",   "DEFAULT": [1.0, 1.0, 1.0, 1.0]},
    {"NAME": "center", "TYPE": "point2D", "DEFAULT": [0.5, 0.5]}
  ]
}*/

vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

void main() {
    vec2 uv = isf_FragNormCoord - center;
    uv.x *= RENDERSIZE.x / RENDERSIZE.y;
    float a = atan(uv.y, uv.x) + TIME * speed;
    float v = step(0.5, fract(a * float(bars) / 6.2831853));
    if (invert) v = 1.0 - v;
    float ring = smoothstep(0.02, 0.0, abs(length(uv) - 0.3));
    vec3 col = hsv2rgb(vec3(hue + length(uv) * 0.5, 0.8, max(v * 0.9, ring))) * tint.rgb;
    gl_FragColor = vec4(col, 1.0);
}
