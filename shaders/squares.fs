/*{
  "DESCRIPTION": "Port of blitbomb scenes/squares.pd: two grids of grey bars. Group A's column spacing is jittered by a noise value that Pd sends every 100 ms (glfo/control-noise); group B is steady. Control patch: pd/squares.pd.",
  "CREDIT": "dnewcome / vlfo",
  "CATEGORIES": ["Generator"],
  "INPUTS": [
    {"NAME": "spread",  "TYPE": "float", "DEFAULT": 60.0, "MIN": 0.0,  "MAX": 400.0},
    {"NAME": "noise",   "TYPE": "float", "DEFAULT": 0.0,  "MIN": -1.0, "MAX": 1.0},
    {"NAME": "offset",  "TYPE": "float", "DEFAULT": -3.0, "MIN": -8.0, "MAX": 8.0},
    {"NAME": "spread2", "TYPE": "float", "DEFAULT": 60.0, "MIN": 0.0,  "MAX": 400.0},
    {"NAME": "offset2", "TYPE": "float", "DEFAULT": 1.0,  "MIN": -8.0, "MAX": 8.0},
    {"NAME": "cols",    "TYPE": "long",  "DEFAULT": 5,    "MIN": 1,    "MAX": 32},
    {"NAME": "rows",    "TYPE": "long",  "DEFAULT": 6,    "MIN": 1,    "MAX": 32},
    {"NAME": "barsize", "TYPE": "point2D", "DEFAULT": [0.3, 0.1]},
    {"NAME": "showA",   "TYPE": "bool",  "DEFAULT": true},
    {"NAME": "showB",   "TYPE": "bool",  "DEFAULT": true},
    {"NAME": "tint",    "TYPE": "color", "DEFAULT": [0.5, 0.5, 0.5, 1.0]}
  ]
}*/

// GEM world units as seen through a glfo card: 8 units tall, 8 * aspect wide,
// origin at the centre, y up. [rectangle 0.3 0.1] is a bar with those half-sizes.

float bars(vec2 p, float spacing, float xoff) {
    float hit = 0.0;
    for (int r = 0; r < rows; r++) {
        for (int c = 0; c < cols; c++) {
            vec2 center = vec2(float(c) * spacing + xoff, float(r));
            vec2 d = abs(p - center) - barsize;
            if (d.x <= 0.0 && d.y <= 0.0) hit = 1.0;
        }
    }
    return hit;
}

void main() {
    float aspect = RENDERSIZE.x / RENDERSIZE.y;
    vec2 p = (isf_FragNormCoord - 0.5) * vec2(8.0 * aspect, 8.0);
    float a = showA ? bars(p, (noise + spread) * 0.01, offset) : 0.0;
    float b = showB ? bars(p, spread2 * 0.01, offset2) : 0.0;
    gl_FragColor = vec4(tint.rgb * max(a, b), 1.0);
}
