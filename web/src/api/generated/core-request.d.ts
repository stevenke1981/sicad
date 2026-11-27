/* eslint-disable */
/**
 * AUTO-GENERATED FILE: do not edit — generated from schemas/cad2/core-request.schema.json.
 * Regenerate with `npm run gen:types` (in web/) after changing the Rust core API (cargo run -- schema).
 */

/**
 * One request, tagged by `op`.
 */
export type CoreRequest =
  | {
      /**
       * Check the completed-document rules (components ≥ 1, parts ≥ 1, root). Default `true`.
       */
      completed?: boolean;
      /**
       * Raw 1.0 or 2.0 document.
       */
      document: {
        [k: string]: unknown;
      };
      op: 'validate';
    }
  | {
      /**
       * Raw 1.0 or 2.0 document.
       */
      document: {
        [k: string]: unknown;
      };
      op: 'solve';
    }
  | {
      /**
       * Raw 1.0 or 2.0 document.
       */
      document: {
        [k: string]: unknown;
      };
      op: 'normalize';
    }
  | {
      /**
       * Raw 1.0 document.
       */
      document: {
        [k: string]: unknown;
      };
      op: 'upgrade';
    }
  | {
      /**
       * Raw 2.0 document.
       */
      document: {
        [k: string]: unknown;
      };
      op: 'downgrade';
    }
  | {
      /**
       * Raw 2.0 document (the current state).
       */
      document: {
        [k: string]: unknown;
      };
      /**
       * `seq` of the op-log entry: the number of entries already in the host's
       * `oplog.jsonl`. Default 0.
       */
      log_seq?: number;
      op: 'apply_patch';
      origin: PatchOrigin;
      /**
       * Raw patch.
       */
      patch: {
        [k: string]: unknown;
      };
      /**
       * Caller-supplied timestamp copied into the op-log entry (the core reads no clock).
       */
      timestamp?: string | null;
    }
  | {
      /**
       * Raw 1.0 or 2.0 document.
       */
      document: {
        [k: string]: unknown;
      };
      op: 'describe';
    }
  | {
      op: 'version';
    };
/**
 * Who submitted a patch: an agent role or `human`.
 */
export type PatchOrigin = 'space_planner' | 'component_smith' | 'joiner' | 'assembler' | 'human';
