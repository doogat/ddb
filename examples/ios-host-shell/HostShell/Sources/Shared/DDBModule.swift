import Foundation

/// Protocol for host-shell feature modules.
/// Each module declares its tables and bootstraps its schema.
protocol DDBModule {
    static var tables: [String] { get }
    static func bootstrap(_ driver: DoogatDriver) throws
}

/// Extract a named column value from a row using the column index list.
func columnValue(_ row: [String], _ columns: [String], _ name: String) -> String {
    guard let idx = columns.firstIndex(of: name), idx < row.count else { return "" }
    return row[idx]
}
