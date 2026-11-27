// A tiny valid CadDocument 2.0 (checked against the real core: validate => valid, no issues).
// Used by the "載入 2.0 範例" button; this is plain document data, not a generated type.
export const SAMPLE_DOCUMENT_2_0: Record<string, unknown> = {
  "schema_version": "2.0",
  "name": "sample-plate",
  "source_prompt": "60x40x5 mm 固定板",
  "unit": "mm",
  "revision": 1,
  "root": "a_main",
  "planner": {
    "planner": "manual",
    "confidence": 1,
    "review_required": false
  },
  "parameters": [
    {
      "name": "thk",
      "value": 5,
      "min": null,
      "max": null,
      "note": "厚"
    }
  ],
  "bindings": [
    {
      "target": "v_plate:thickness",
      "expr": "thk"
    }
  ],
  "components": [
    {
      "id": "v_plate",
      "kind": "volume",
      "shape": {
        "kind": "plate",
        "length": 60,
        "width": 40,
        "thickness": 5
      },
      "label": "固定板",
      "provenance": null
    }
  ],
  "constraints": [
    {
      "id": "k_root",
      "kind": "fixed",
      "a": "v_plate.origin",
      "b": null,
      "value": null,
      "u": null,
      "v": null,
      "frame": {
        "origin": [
          0,
          0,
          0
        ],
        "x_axis": "+x",
        "z_axis": "+z"
      }
    }
  ],
  "parts": [
    {
      "id": "p_plate",
      "name": "固定板",
      "material": "aluminum",
      "quantity": 1,
      "source": "made",
      "body": [
        {
          "op": "add",
          "component": "v_plate"
        }
      ],
      "fillets": [],
      "chamfers": []
    }
  ],
  "assemblies": [
    {
      "id": "a_main",
      "name": "固定板",
      "children": [
        "p_plate"
      ]
    }
  ]
};
