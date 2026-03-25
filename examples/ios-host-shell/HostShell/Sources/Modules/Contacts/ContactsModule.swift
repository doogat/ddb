import Foundation

struct ContactsModule: DDBModule {
    static let tables = ["contact"]

    static func bootstrap(_ driver: DoogatDriver) throws {
        _ = try driver.executeSql(sql: """
            CREATE TABLE IF NOT EXISTS contact (
                name TEXT NOT NULL,
                relationship TEXT,
                email TEXT
            )
        """)
    }
}
