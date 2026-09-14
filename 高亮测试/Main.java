package sample;

import java.util.List;

@Deprecated
public class Main {
    private static final int MAX_RETRIES = 3;

    static String greet(String name) {
        return "Hello, " + name + "!";
    }

    public static void main(String[] args) {
        List<String> names = List.of("Zcv", "Java");
        for (String name : names) {
            System.out.println(greet(name));
        }
        System.out.println(MAX_RETRIES);
    }
}
