import Pbf from 'pbf';

/**
 * Minimal browser OSM PBF reader (no Node streams, no GDAL).
 * Decodes the OSM PBF container (BlobHeader/Blob), zlib-decompresses blobs
 * with DecompressionStream, and reads ways + dense nodes from PrimitiveBlocks.
 * Used for the pre-loaded "Use example data" release asset.
 */

export type OsmPbfWay = { id: number; refs: number[]; tags: Record<string, string> };
export type OsmPbfResult = {
  nodes: Map<number, [number, number]>;
  ways: OsmPbfWay[];
};

/** Zigzag decode for arbitrary JS numbers (not just 32-bit). */
function zigzag(n: number): number {
  return n % 2 === 0 ? n / 2 : -(n + 1) / 2;
}

/** Parse a BlobHeader: field 1 type (string), field 2 indexdata (skip), field 3 datasize (varint). */
function readBlobHeader(pbf: Pbf): { type: string; datasize: number } {
  let type = '';
  let datasize = 0;
  while (pbf.pos < pbf.length) {
    const tag = pbf.readVarint();
    const field = tag >> 3;
    const wire = tag & 7;
    if (field === 1 && wire === 2) {
      type = pbf.readString();
    } else if (field === 3 && wire === 0) {
      datasize = pbf.readVarint();
    } else {
      pbf.skip(tag);
    }
  }
  return { type, datasize };
}

/** Parse a Blob: field 1 raw, field 3 zlib_data (decompressed async), field 7 zstd_data. */
async function readBlob(pbf: Pbf): Promise<Uint8Array> {
  while (pbf.pos < pbf.length) {
    const tag = pbf.readVarint();
    const field = tag >> 3;
    const wire = tag & 7;
    if (field === 1 && wire === 2) {
      return pbf.readBytes();
    }
    if (field === 3 && wire === 2) {
      const bytes = pbf.readBytes();
      return inflate(bytes);
    }
    if (field === 7 && wire === 2) {
      const bytes = pbf.readBytes();
      return zstdDecompress(bytes);
    }
    pbf.skip(tag);
  }
  throw new Error('OSM PBF blob has no data payload');
}

async function inflate(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([new Uint8Array(bytes)]).stream().pipeThrough(new DecompressionStream('deflate'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

// zstd blobs appear in PBF files written by recent libosmium (Geofabrik default).
let zstdDecompress: (bytes: Uint8Array) => Promise<Uint8Array> = async (bytes) => {
  const module = await import('fzstd');
  return module.decompress(bytes);
};

/**
 * Parse a PrimitiveBlock (OSMData) into nodes and ways.
 * Field layout: 1 stringtable, 2 primitivegroup (nodes/dense/ways),
 * 17 granularity, 19 lat_offset, 20 lon_offset.
 */
function parsePrimitiveBlock(pbf: Pbf, label: string): OsmPbfResult {
  const strings: string[] = [];
  const nodes = new Map<number, [number, number]>();
  const ways: OsmPbfWay[] = [];
  let granularity = 100;
  let latOffset = 0;
  let lonOffset = 0;

  const blockEnd = pbf.length;
  while (pbf.pos < blockEnd) {
    try {
      const tag = pbf.readVarint();
      const field = tag >> 3;
      const wire = tag & 7;
    if (field === 1 && wire === 2) {
      const len = pbf.readVarint();
      const end = pbf.pos + len;
      // Stringtable entries are string *messages*: tag (field 1, wire 2) +
      // length + payload, not raw length-prefixed strings.
      while (pbf.pos < end) {
        const st = pbf.readVarint();
        if ((st >> 3) === 1 && (st & 7) === 2) strings.push(pbf.readString());
        else pbf.skip(st);
      }
    } else if (field === 2 && wire === 2) {
      const len = pbf.readVarint();
      const groupEnd = pbf.pos + len;
      while (pbf.pos < groupEnd) {
        const gtag = pbf.readVarint();
        const gfield = gtag >> 3;
        const gwire = gtag & 7;
        if (gfield === 2 && gwire === 2) {
          // dense nodes: collect packed delta arrays, then walk together
          const glen = pbf.readVarint();
          const dEnd = pbf.pos + glen;
          let ids: number[] = [];
          let lats: number[] = [];
          let lons: number[] = [];
          let keysVals: number[] = [];
          while (pbf.pos < dEnd) {
            const dtag = pbf.readVarint();
            const df = dtag >> 3;
            const dw = dtag & 7;
            pbf.type = dw; // readPacked* consult this.type
            if (df === 1 && dw === 2) ids = pbf.readPackedSVarint([]);
            else if (df === 8 && dw === 2) lats = pbf.readPackedSVarint([]);
            else if (df === 9 && dw === 2) lons = pbf.readPackedSVarint([]);
            else if (df === 10 && dw === 2) keysVals = pbf.readPackedVarint([]);
            else pbf.skip(dtag);
          }
          // Decode zigzag deltas and store every node (tags are not needed
          // for geometry; way tags carry the highway class).
          let id = 0;
          let lat = 0;
          let lon = 0;
          let ki = 0;
          for (let i = 0; i < ids.length; i += 1) {
            id += ids[i];
            lat += lats[i] ?? 0;
            lon += lons[i] ?? 0;
            // keys_vals: each node starts with a 0 separator, then optional
            // key/value string-table index pairs until the next 0.
            if (ki < keysVals.length && keysVals[ki] === 0) ki += 1;
            while (ki < keysVals.length && keysVals[ki] !== 0) ki += 2;
            const lonDeg = (lonOffset + granularity * lon) / 1e9;
            const latDeg = (latOffset + granularity * lat) / 1e9;
            nodes.set(id, [lonDeg, latDeg]);
          }
        } else if (gfield === 3 && gwire === 2) {
          // way
          const glen = pbf.readVarint();
          const wEnd = pbf.pos + glen;
          let id = 0;
          const keys: number[] = [];
          const vals: number[] = [];
          const refs: number[] = [];
          let ref = 0;
          while (pbf.pos < wEnd) {
            const wt = pbf.readVarint();
            const wf = wt >> 3;
            const ww = wt & 7;
            if (wf === 1 && ww === 0) {
              id = pbf.readVarint();
            } else if (wf === 2 && ww === 2) {
              const l = pbf.readVarint();
              const e = pbf.pos + l;
              while (pbf.pos < e) keys.push(pbf.readVarint());
            } else if (wf === 3 && ww === 2) {
              const l = pbf.readVarint();
              const e = pbf.pos + l;
              while (pbf.pos < e) vals.push(pbf.readVarint());
            } else if (wf === 8 && ww === 2) {
              const l = pbf.readVarint();
              const e = pbf.pos + l;
              while (pbf.pos < e) {
                ref += zigzag(pbf.readVarint());
                refs.push(ref);
              }
            } else {
              pbf.skip(wt);
            }
          }
          const tags: Record<string, string> = {};
          for (let i = 0; i < keys.length && i < vals.length; i += 1) {
            tags[strings[keys[i]]] = strings[vals[i]];
          }
          ways.push({ id, refs, tags });
        } else {
          pbf.skip(gtag);
        }
      }
    } else if (field === 17 && wire === 0) {
      granularity = pbf.readVarint();
    } else if (field === 19 && wire === 0) {
      latOffset = pbf.readVarint();
    } else if (field === 20 && wire === 0) {
      lonOffset = pbf.readVarint();
    } else {
      pbf.skip(tag);
    }
    } catch (reason) {
      throw new Error(`OSM PBF parse failed in ${label} at block offset ${pbf.pos}: ${reason instanceof Error ? reason.message : String(reason)}`);
    }
  }
  return { nodes, ways };
}

/** Parse a full OSM PBF byte array (async: zlib/zstd blobs are decompressed). */
export async function parseOsmPbf(bytes: Uint8Array): Promise<OsmPbfResult> {
  const nodes = new Map<number, [number, number]>();
  const ways: OsmPbfWay[] = [];
  let offset = 0;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  while (offset + 4 <= bytes.byteLength) {
    const headerLength = view.getUint32(offset);
    offset += 4;
    const headerPbf = new Pbf(bytes.subarray(offset, offset + headerLength));
    const { type, datasize } = readBlobHeader(headerPbf);
    offset += headerLength;
    const blobPbf = new Pbf(bytes.subarray(offset, offset + datasize));
    const payload = await readBlob(blobPbf);
    offset += datasize;
    if (type === 'OSMData') {
      const block = parsePrimitiveBlock(new Pbf(payload), type);
      for (const [id, coord] of block.nodes) nodes.set(id, coord);
      ways.push(...block.ways);
    }
  }
  return { nodes, ways };
}
