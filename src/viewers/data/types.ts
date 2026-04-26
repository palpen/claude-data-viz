export interface Column {
  key: string;
}

export interface ParseError {
  row: number;
  message: string;
}

export interface Dataset {
  columns: Column[];
  rows: string[][];
  parseErrors: ParseError[];
  // null = full file shown.
  // { totalRows: number } = whole file was scanned, displayed rows are a prefix; total is exact.
  // { totalRows: null } = only a byte-bounded preview of the file was read; total is unknown.
  truncation: { totalRows: number | null } | null;
}
